//! Hands a take to an editor: the live edit as a timeline that references
//! the recorded files, so any cut can be changed afterwards.
//!
//! * `<take>.fcpxml` for Final Cut Pro and DaVinci Resolve: a multicam clip
//!   with every recorded angle (plus the program), and a project with the
//!   live cuts on it.
//! * `<take> (Premiere).xml`: a sequence with the live edit on V1 and every
//!   angle on its own (disabled) track above, ready to swap shots.
//! * `Chapters.txt`: the markers as a YouTube chapter list.
//!
//! Scenes that show a single source full frame cut to that source's file;
//! composite scenes (side by side, picture in picture...) use the program.
//!
//! A close-up saved as a crop of its wide camera has no file of its own:
//! its angle is the wide camera's file, zoomed and moved (and trimmed, when
//! its shape isn't the timeline's) to frame the same shot (`framing`).
//! Final Cut gets its moves as keyframes; Premiere and the Resolve script
//! get one steady framing per stretch, a move cutting to where it ended
//! (`crop_steps`).

use super::switcher::SwitchEvent;
use crate::media::{Ns, NS_PER_SEC};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExportFile {
    pub path: PathBuf,
    /// Angle name (source name, or "Program").
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub has_audio: bool,
    /// Source index, or None for the program.
    pub source: Option<usize>,
    /// First frame's timecode (frames since midnight at `fps`).
    pub tc_frame: Option<u32>,
    /// Where the file ends (from the start of the take) when that's before
    /// the take did: its source left the show while recording.
    pub length: Option<Ns>,
    /// The program without graphics: the shots of more than one camera cut
    /// to it, so the whole edit is free of graphics like the cameras' own
    /// files, ready for new titles.
    pub clean: bool,
    /// Where the file starts (from the start of the take): 0, unless its
    /// source joined or restarted during the take and got a file of its
    /// own from then on.
    #[serde(default)]
    pub start: Ns,
    /// A close-up saved as a crop of a wide camera: `path` (and the size,
    /// rate and timecode) are the wide camera's file, and this says which
    /// part of it the close-up showed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

/// Which part of a wide camera's file a close-up showed.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Crop {
    /// The wide camera (source identity).
    pub of: usize,
    /// The close-up's own picture size (the shot's shape).
    pub size: (u32, u32),
    /// Where the shot was: (time from the start of the take, x, y, width
    /// and height as fractions of the wide picture), in time order, from
    /// the start (see `ptz::ShotLog`).
    #[serde(default)]
    pub keys: Vec<(Ns, [f32; 4])>,
}

impl Crop {
    /// The shot at `at` (from the start of the take).
    pub fn rect_at(&self, at: Ns) -> [f32; 4] {
        self.keys.iter().take_while(|k| k.0 <= at).last().or(self.keys.first()).map(|k| k.1).unwrap_or([0.0, 0.0, 1.0, 1.0])
    }
}

/// Keys further apart than this are separate moves (`ptz::SHOT_LOG_GAP`
/// apart while it moves, plus a frame).
const MOVE_GAP: Ns = 300_000_000;

/// The shot as steady framings (from when, which part) for editors that
/// get no keyframes: each move becomes a cut, at its start, to where it
/// came to rest.
pub fn crop_steps(keys: &[(Ns, [f32; 4])]) -> Vec<(Ns, [f32; 4])> {
    let mut out: Vec<(Ns, [f32; 4])> = Vec::new();
    let mut i = 0;
    while i < keys.len() {
        let mut j = i;
        while j + 1 < keys.len() && keys[j + 1].0 - keys[j].0 <= MOVE_GAP {
            j += 1;
        }
        if out.last().is_none_or(|l| l.1 != keys[j].1) {
            out.push((keys[i].0, keys[j].1));
        }
        i = j + 1;
    }
    out
}

/// How an editor frames part of a wide picture to show what the close-up
/// showed, starting from the editor's own fit of the whole picture to the
/// timeline (Final Cut's, Premiere's and Resolve's default for a file
/// another size): `zoom` times bigger, its centre moved by `x`, `y`
/// (timeline pixels, right and up), and, when the close-up's shape isn't
/// the timeline's, `crop` pixels trimmed off its left, right, top and
/// bottom first (in timeline pixels of the fitted picture), so it shows
/// with bars as the close-up's own file would.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub zoom: f64,
    pub x: f64,
    pub y: f64,
    pub crop: Option<[f64; 4]>,
    /// The whole wide picture fitted to the timeline: its size there, and
    /// its scale from its own pixels.
    pub fitted: (f64, f64),
    pub fit: f64,
}

/// The `Framing` of `rect` (fractions of a `wide`-sized picture, of the
/// close-up's `shot` shape) on a `timeline`-sized timeline.
pub fn framing(rect: [f32; 4], wide: (u32, u32), shot: (u32, u32), timeline: (u32, u32)) -> Framing {
    let (w, h) = (wide.0.max(1) as f64, wide.1.max(1) as f64);
    let (tw, th) = (timeline.0.max(1) as f64, timeline.1.max(1) as f64);
    let (sw, sh) = (shot.0.max(1) as f64, shot.1.max(1) as f64);
    let [rx, ry, rw, rh] = rect.map(|v| v as f64);
    let fit = (tw / w).min(th / h);
    // Where the close-up's own file would show: fitted, in the middle.
    let shot_fit = (tw / sw).min(th / sh);
    let (bw, bh) = (sw * shot_fit, sh * shot_fit);
    // The part, in the fitted picture.
    let (pw, ph) = ((rw * w * fit).max(1e-9), (rh * h * fit).max(1e-9));
    let zoom = (bw / pw).min(bh / ph);
    let cx = (rx + rw / 2.0 - 0.5) * w * fit;
    let cy = (ry + rh / 2.0 - 0.5) * h * fit;
    let fills = bw >= tw - 0.5 && bh >= th - 0.5;
    let crop = (!fills).then(|| [rx * w * fit, (1.0 - rx - rw) * w * fit, ry * h * fit, (1.0 - ry - rh) * h * fit].map(|v| v.max(0.0)));
    Framing { zoom, x: -zoom * cx, y: zoom * cy, crop, fitted: (w * fit, h * fit), fit }
}

/// A number for the XML: four decimals at most, no trailing zeros.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

impl Framing {
    /// Final Cut's `adjust-transform`: position in percent of the
    /// timeline's height from its centre (up is positive), scale on top of
    /// its fit.
    pub fn fcp_transform(&self, timeline_h: u32) -> String {
        let th = timeline_h.max(1) as f64;
        format!("position=\"{} {}\" scale=\"{z} {z}\"", num(self.x / th * 100.0), num(self.y / th * 100.0), z = num(self.zoom))
    }

    /// Final Cut's position keyframe value.
    fn fcp_position(&self, timeline_h: u32) -> String {
        let th = timeline_h.max(1) as f64;
        format!("{} {}", num(self.x / th * 100.0), num(self.y / th * 100.0))
    }

    /// Final Cut's `trim-rect`: percent of the file's own picture height.
    pub fn fcp_trim(&self) -> Option<String> {
        let c = self.crop?;
        let pct = |v: f64| num(v / self.fitted.1.max(1e-9) * 100.0);
        Some(format!("left=\"{}\" top=\"{}\" right=\"{}\" bottom=\"{}\"", pct(c[0]), pct(c[2]), pct(c[1]), pct(c[3])))
    }

    /// Premiere's Basic Motion (scale in percent of the file's own pixels;
    /// centre as a fraction of the frame from its middle, down positive)
    /// and Crop (percent of the picture's width and height off each side).
    pub fn premiere_filters(&self, timeline: (u32, u32)) -> String {
        let (tw, th) = (timeline.0.max(1) as f64, timeline.1.max(1) as f64);
        let param = |id: &str, name: &str, value: String| format!("<parameter><parameterid>{id}</parameterid><name>{name}</name>{value}</parameter>");
        let mut x = format!(
            "<filter><effect><name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>{}{}</effect></filter>",
            param("scale", "Scale", format!("<valuemin>0</valuemin><valuemax>1000</valuemax><value>{}</value>", num(self.zoom * self.fit * 100.0))),
            param("center", "Center", format!("<value><horiz>{}</horiz><vert>{}</vert></value>", num(self.x / tw), num(-self.y / th)))
        );
        if let Some(c) = self.crop {
            let (fw, fh) = (self.fitted.0.max(1e-9), self.fitted.1.max(1e-9));
            let side = |id: &str, name: &str, v: f64| param(id, name, format!("<valuemin>0</valuemin><valuemax>100</valuemax><value>{}</value>", num(v * 100.0)));
            x += &format!(
                "<filter><effect><name>Crop</name><effectid>crop</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>{}{}{}{}</effect></filter>",
                side("left", "left", c[0] / fw),
                side("right", "right", c[1] / fw),
                side("top", "top", c[2] / fh),
                side("bottom", "bottom", c[3] / fh)
            );
        }
        x
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// Program frame (from the start of the take) where this shot begins.
    pub frame: i64,
    /// Source index, or None for the program.
    pub angle: Option<usize>,
    pub scene: String,
}

/// One shot of the live edit as an editor places it: program frames
/// `start..end`, from frames `src_in..src_out` of `files[file]` (in that
/// file's own frames). The Premiere timeline and the Resolve script's take
/// file are both written from these, so they agree.
#[derive(Clone, Debug, PartialEq)]
pub struct EditShot {
    pub start: i64,
    pub end: i64,
    pub file: usize,
    pub src_in: i64,
    pub src_out: i64,
    pub scene: String,
    /// A close-up saved as a crop: the part of the file it shows here.
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct TakeExport {
    pub name: String,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Length in program frames.
    pub frames: i64,
    pub files: Vec<ExportFile>,
    pub cuts: Vec<Cut>,
    /// (program frame, title)
    pub markers: Vec<(i64, String)>,
}

fn ns_to_frame(ns: Ns, fps: u32) -> i64 {
    ((ns as i128 * fps as i128 + NS_PER_SEC as i128 / 2).div_euclid(NS_PER_SEC as i128)) as i64
}

/// Program frame `frame`'s time from the start of the take.
fn ns_to_frame_ns(frame: i64, fps: u32) -> Ns {
    (frame as i128 * NS_PER_SEC as i128 / fps.max(1) as i128) as Ns
}

/// The live edit from the switch history. `t0` is the start of the files;
/// `latency` is how much later a switch shows in the program than the
/// button press. Mixes cut halfway through.
#[allow(clippy::too_many_arguments)]
pub fn cuts_from_switches(
    first_scene: &str,
    first_angle: Option<usize>,
    events: &[SwitchEvent],
    t0: Ns,
    latency: Ns,
    fps: u32,
    frames: i64,
    recorded: &HashSet<usize>,
) -> Vec<Cut> {
    let pick = |a: Option<usize>| a.filter(|i| recorded.contains(i));
    let mut cuts = vec![Cut { frame: 0, angle: pick(first_angle), scene: first_scene.to_string() }];
    for e in events {
        let mut at = e.at - latency - t0;
        // Mid-way through a transition (a stinger's switch is logged at its cut).
        if !matches!(e.mode, crate::config::TransitionMode::Cut | crate::config::TransitionMode::Stinger) {
            at += e.duration_ms as i64 * 1_000_000 / 2;
        }
        let frame = ns_to_frame(at, fps).clamp(0, frames.max(1) - 1);
        let cut = Cut { frame, angle: pick(e.angle), scene: e.scene.clone() };
        // A later switch at the same frame wins.
        while cuts.last().is_some_and(|c| c.frame >= frame) && cuts.len() > 1 {
            cuts.pop();
        }
        if cuts.last().is_some_and(|c| c.frame >= frame) {
            cuts[0] = Cut { frame: 0, ..cut };
            continue;
        }
        if cuts.last().is_some_and(|c| c.angle == cut.angle) {
            continue; // same picture source: no edit needed
        }
        cuts.push(cut);
    }
    cuts
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `file://` URL with everything but safe characters percent-encoded.
fn file_url(path: &Path, host: &str) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    file_url_of(&abs.to_string_lossy(), cfg!(windows), host)
}

/// The URL of an absolute path, written the Windows way (`C:\Videos\a.mp4`
/// → `file:///C:/Videos/a.mp4`; `\\server\share\a.mp4` →
/// `file://server/share/a.mp4`) or the Unix way.
fn file_url_of(path: &str, windows: bool, host: &str) -> String {
    let mut host = host.to_string();
    let mut path = path.to_string();
    if windows {
        path = path.replace('\\', "/");
        if let Some(rest) = path.strip_prefix("//?/UNC/") {
            path = format!("//{rest}");
        } else if let Some(rest) = path.strip_prefix("//?/") {
            path = rest.to_string();
        }
        if let Some(unc) = path.strip_prefix("//") {
            // A network share: the server is the URL's host.
            let (server, rest) = unc.split_once('/').unwrap_or((unc, ""));
            host = server.to_string();
            path = format!("/{rest}");
        } else if !path.starts_with('/') {
            path.insert(0, '/');
        }
    }
    let mut out = format!("file://{host}");
    let bytes = path.as_bytes();
    // A drive letter's colon stays as it is (`/C:/`).
    let drive = windows && bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(*b as char),
            b':' if drive && i == 2 => out.push(':'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Rational seconds as FCPXML writes them ("1001/30000s", "5s").
fn rt(num: i64, den: i64) -> String {
    fn gcd(a: i64, b: i64) -> i64 {
        if b == 0 {
            a.abs()
        } else {
            gcd(b, a % b)
        }
    }
    if num == 0 {
        return "0s".into();
    }
    let g = gcd(num, den).max(1);
    let (n, d) = (num / g, den / g);
    if d == 1 {
        format!("{n}s")
    } else {
        format!("{n}/{d}s")
    }
}

fn timecode_string(frame: i64, fps: u32) -> String {
    let fps = fps.max(1) as i64;
    let f = frame.rem_euclid(fps);
    let s = frame.div_euclid(fps);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f)
}

impl TakeExport {
    fn program(&self) -> Option<&ExportFile> {
        self.files.iter().find(|f| f.source.is_none())
    }

    fn file_for(&self, angle: Option<usize>) -> Option<&ExportFile> {
        if angle.is_none() {
            if let Some(clean) = self.files.iter().find(|f| f.clean) {
                return Some(clean);
            }
        }
        self.files.iter().find(|f| f.source == angle).or_else(|| self.program())
    }

    /// The wide camera's file a close-up saved as a crop is cut from.
    pub fn wide_of(&self, f: &ExportFile) -> Option<&ExportFile> {
        f.crop.as_ref()?;
        self.files.iter().find(|g| g.crop.is_none() && g.path == f.path)
    }

    /// The file that's really there: the wide camera's, for a crop.
    pub fn media_of<'a>(&'a self, f: &'a ExportFile) -> &'a ExportFile {
        self.wide_of(f).unwrap_or(f)
    }

    /// Where a file ends (from the start of the take), if before the take
    /// did. A crop: when its close-up or its wide camera's file did.
    fn length_of(&self, f: &ExportFile) -> Option<Ns> {
        let wide = self.wide_of(f).and_then(|w| w.length);
        match (f.length, wide) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Length of a file in its own frames (the take's length, unless its
    /// source left early or joined late).
    pub fn file_frames(&self, f: &ExportFile) -> i64 {
        let start = self.file_start(f);
        let full = ((self.frames - start).max(0) * f.fps as i64 + self.fps as i64 / 2) / self.fps as i64;
        self.length_of(f).map_or(full, |ns| ns_to_frame(ns - f.start, f.fps).clamp(0, full))
    }

    /// Where a file starts, in program frames.
    pub fn file_start(&self, f: &ExportFile) -> i64 {
        ns_to_frame(f.start, self.fps).clamp(0, self.frames)
    }

    /// Where a file ends, in program frames.
    fn file_end(&self, f: &ExportFile) -> i64 {
        self.length_of(f).map_or(self.frames, |ns| ns_to_frame(ns, self.fps).clamp(0, self.frames))
    }

    /// Program frame `seq_frame` in `f`'s own frames.
    pub fn in_file(&self, f: &ExportFile, seq_frame: i64) -> i64 {
        ((seq_frame - self.file_start(f)) * f.fps as i64 + self.fps as i64 / 2) / self.fps as i64
    }

    /// A crop's steady framings over program frames `a..b`: (from, to,
    /// part). One piece, the whole way, for any other file.
    pub fn pieces(&self, f: &ExportFile, a: i64, b: i64) -> Vec<(i64, i64, Option<[f32; 4]>)> {
        let Some(c) = &f.crop else { return vec![(a, b, None)] };
        let steps = crop_steps(&c.keys);
        let mut out = Vec::new();
        let mut at = a;
        let mut rect = steps.first().map(|s| s.1).unwrap_or_else(|| c.rect_at(ns_to_frame_ns(a, self.fps)));
        for &(t, r) in &steps {
            let frame = ns_to_frame(t, self.fps);
            if frame <= a {
                rect = r;
                continue;
            }
            if frame >= b {
                break;
            }
            if r != rect {
                out.push((at, frame, Some(rect)));
                at = frame;
                rect = r;
            }
        }
        if b > at {
            out.push((at, b, Some(rect)));
        }
        out
    }

    /// How `rect` of a crop's wide file frames the close-up's shot here.
    pub fn framing_of(&self, f: &ExportFile, rect: [f32; 4]) -> Option<Framing> {
        let c = f.crop.as_ref()?;
        Some(framing(rect, (f.width, f.height), c.size, (self.width, self.height)))
    }

    /// Shots: (start frame, end frame, file).
    fn shots(&self) -> Vec<(i64, i64, &ExportFile, &str)> {
        let mut out = Vec::new();
        for (i, c) in self.cuts.iter().enumerate() {
            let end = self.cuts.get(i + 1).map(|n| n.frame).unwrap_or(self.frames);
            if end > c.frame {
                if let Some(f) = self.file_for(c.angle) {
                    out.push((c.frame, end, f, c.scene.as_str()));
                }
            }
        }
        out
    }

    /// The live edit, shot by shot (see `EditShot`). A source that left
    /// the show: its shot stops with its file (a gap till the next; the
    /// program's sound carries on below). A source with more than one
    /// file (it restarted during the take): each part of the shot comes
    /// from the file that has it.
    pub fn edit(&self) -> Vec<EditShot> {
        let mut out = Vec::new();
        for (start, end, f, scene) in self.shots() {
            // The files of that angle (one, unless it restarted).
            let files: Vec<usize> = match f.source {
                Some(src) => (0..self.files.len()).filter(|&i| self.files[i].source == Some(src)).collect(),
                None => vec![self.files.iter().position(|g| std::ptr::eq(g, f)).unwrap_or(0)],
            };
            for i in files {
                let g = &self.files[i];
                let (a, b) = (start.max(self.file_start(g)), end.min(self.file_end(g)));
                if b <= a {
                    continue;
                }
                // (A crop: a piece for each steady framing.)
                for (a, b, rect) in self.pieces(g, a, b) {
                    let m = self.media_of(g);
                    out.push(EditShot { start: a, end: b, file: i, src_in: self.in_file(m, a), src_out: self.in_file(m, b), scene: scene.to_string(), rect });
                }
            }
        }
        out
    }

    /// The angles of the multicam: one per recorded source (all of its
    /// files), and one each for the program files. File indices.
    fn angles(&self) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        for (i, f) in self.files.iter().enumerate() {
            match f.source.and_then(|s| out.iter_mut().find(|a| self.files[a[0]].source == Some(s))) {
                Some(a) => a.push(i),
                None => out.push(vec![i]),
            }
        }
        out
    }

    /// The file whose sound goes under the edit: the program's (the mix).
    pub fn sound(&self) -> Option<&ExportFile> {
        self.files.iter().find(|f| f.source.is_none() && f.has_audio).or_else(|| self.files.iter().find(|f| f.has_audio))
    }
}

/// Final Cut Pro / DaVinci Resolve.
pub fn fcpxml(t: &TakeExport) -> String {
    let fps = t.fps as i64;
    let mut formats: Vec<(u32, u32, u32)> = vec![(t.width, t.height, t.fps)];
    for f in &t.files {
        if !formats.contains(&(f.width, f.height, f.fps)) {
            formats.push((f.width, f.height, f.fps));
        }
    }
    let fmt_id = |w: u32, h: u32, r: u32| format!("f{}", formats.iter().position(|x| *x == (w, h, r)).unwrap_or(0) + 1);
    // Every file starts at the same instant; the multicam's clock is the
    // program's timecode.
    let prog_tc = t.program().and_then(|p| p.tc_frame).unwrap_or(0) as i64;
    let mc_start = (prog_tc, fps);
    let asset_start = |f: &ExportFile| (f.tc_frame.unwrap_or(0) as i64, f.fps as i64);
    // A crop's asset is its wide camera's file.
    let asset_id = |i: usize| {
        let f = &t.files[i];
        let m = t.media_of(f);
        format!("a{}", t.files.iter().position(|g| std::ptr::eq(g, m)).unwrap_or(i) + 1)
    };

    let mut x = String::new();
    x += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.9\">\n  <resources>\n";
    for (w, h, r) in &formats {
        let _ = writeln!(x, "    <format id=\"{}\" frameDuration=\"{}\" width=\"{w}\" height=\"{h}\" colorSpace=\"1-1-1 (Rec. 709)\"/>", fmt_id(*w, *h, *r), rt(1, *r as i64));
    }
    for (i, f) in t.files.iter().enumerate() {
        if f.crop.is_some() && t.wide_of(f).is_some() {
            continue;
        }
        let (sn, sd) = asset_start(f);
        let audio = if f.has_audio { " hasAudio=\"1\" audioSources=\"1\" audioChannels=\"2\" audioRate=\"48000\"" } else { "" };
        let _ = writeln!(
            x,
            "    <asset id=\"a{}\" name=\"{}\" start=\"{}\" duration=\"{}\" hasVideo=\"1\" format=\"{}\"{audio} src=\"{}\"/>",
            i + 1,
            esc(&f.path.file_stem().unwrap_or_default().to_string_lossy()),
            rt(sn, sd),
            rt(t.file_frames(f), f.fps as i64),
            fmt_id(f.width, f.height, f.fps),
            esc(&file_url(&f.path, ""))
        );
    }
    // File `i` full length from where it starts (`head` goes before its
    // name: a lane; `tail` after its format). A crop's carries its framing:
    // keyframed when it fills the frame, else a clip per steady framing
    // (trimmed to the close-up's shape).
    let full_clip = |i: usize, head: &str, tail: &str, indent: &str| -> String {
        let f = &t.files[i];
        let (sn, sd) = asset_start(f);
        let el = |offset: i64, start: i64, dur: i64, inner: Option<String>| {
            let open = format!(
                "{indent}<asset-clip ref=\"{}\"{head} name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" format=\"{}\" tcFormat=\"NDF\"{tail}",
                asset_id(i),
                esc(&f.name),
                rt(prog_tc + offset, fps),
                rt(sn + start, sd),
                rt(dur, f.fps as i64),
                fmt_id(f.width, f.height, f.fps)
            );
            match inner {
                None => format!("{open}/>\n"),
                Some(inner) => format!("{open}>\n{inner}{indent}</asset-clip>\n"),
            }
        };
        let (from, to) = (t.file_start(f), t.file_end(f));
        let Some(c) = f.crop.as_ref().filter(|_| t.wide_of(f).is_some()) else {
            return el(from, 0, t.file_frames(f), None);
        };
        // (A close-up that joined later starts part way into the file.)
        let m = t.media_of(f);
        let skip = t.in_file(m, from);
        let first = t.framing_of(f, c.rect_at(f.start)).unwrap_or_else(|| framing([0.0, 0.0, 1.0, 1.0], (1, 1), (1, 1), (1, 1)));
        if first.crop.is_none() {
            // The shot over the file: where it is at the start, then each move.
            let end_ns = f.start + t.file_frames(f) * NS_PER_SEC / f.fps.max(1) as i64;
            let mut keys = vec![(f.start, c.rect_at(f.start))];
            keys.extend(c.keys.iter().filter(|k| k.0 > f.start && k.0 < end_ns).copied());
            let mut inner = format!("{indent}  <adjust-transform {}", first.fcp_transform(t.height));
            if keys.windows(2).any(|w| w[0].1 != w[1].1) {
                let time = |at: Ns| rt(sn + ns_to_frame(at - m.start, f.fps), sd);
                let frames: Vec<(String, Framing)> = keys.iter().filter_map(|k| Some((time(k.0), t.framing_of(f, k.1)?))).collect();
                inner += ">\n";
                let _ = writeln!(inner, "{indent}    <param name=\"position\">\n{indent}      <keyframeAnimation>");
                for (at, fr) in &frames {
                    let _ = writeln!(inner, "{indent}        <keyframe time=\"{at}\" value=\"{}\"/>", fr.fcp_position(t.height));
                }
                let _ = writeln!(inner, "{indent}      </keyframeAnimation>\n{indent}    </param>\n{indent}    <param name=\"scale\">\n{indent}      <keyframeAnimation>");
                for (at, fr) in &frames {
                    let _ = writeln!(inner, "{indent}        <keyframe time=\"{at}\" value=\"{z} {z}\"/>", z = num(fr.zoom));
                }
                let _ = writeln!(inner, "{indent}      </keyframeAnimation>\n{indent}    </param>\n{indent}  </adjust-transform>");
            } else {
                inner += "/>\n";
            }
            return el(from, skip, t.file_frames(f), Some(inner));
        }
        let mut out = String::new();
        for (a, b, rect) in t.pieces(f, from, to) {
            let Some(fr) = rect.and_then(|r| t.framing_of(f, r)) else { continue };
            let mut inner = String::new();
            if let Some(trim) = fr.fcp_trim() {
                let _ = writeln!(inner, "{indent}  <adjust-crop mode=\"trim\">\n{indent}    <trim-rect {trim}/>\n{indent}  </adjust-crop>");
            }
            let _ = writeln!(inner, "{indent}  <adjust-transform {}/>", fr.fcp_transform(t.height));
            let (sa, sb) = (t.in_file(m, a), t.in_file(m, b));
            out += &el(a, sa, sb - sa, Some(inner));
        }
        out
    };
    let _ = writeln!(x, "    <media id=\"m1\" name=\"{} Multicam\">", esc(&t.name));
    let _ = writeln!(x, "      <multicam format=\"{}\" tcStart=\"{}\" tcFormat=\"NDF\">", fmt_id(t.width, t.height, t.fps), rt(mc_start.0, mc_start.1));
    // One angle per source: a source that restarted has its files one
    // after the other in its angle, each where it starts.
    let angles = t.angles();
    for (k, files) in angles.iter().enumerate() {
        let _ = writeln!(x, "        <mc-angle name=\"{}\" angleID=\"angle{k}\">", esc(&t.files[files[0]].name));
        for &i in files {
            x += &full_clip(i, "", "", "          ");
        }
        x += "        </mc-angle>\n";
    }
    let angle_of = |i: usize| angles.iter().position(|a| a.contains(&i)).unwrap_or(0);
    x += "      </multicam>\n    </media>\n  </resources>\n";
    let _ = writeln!(x, "  <library>\n    <event name=\"Cutaway\">\n      <project name=\"{}\">", esc(&t.name));
    let _ = writeln!(
        x,
        "        <sequence format=\"{}\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\" audioLayout=\"stereo\" audioRate=\"48k\">\n          <spine>",
        fmt_id(t.width, t.height, t.fps),
        rt(t.frames, fps)
    );
    let audio_angle = t.files.iter().position(|f| f.source.is_none() && f.has_audio).or_else(|| t.files.iter().position(|f| f.has_audio)).map(angle_of);
    for (start, end, f, scene) in t.shots() {
        let angle = angle_of(t.files.iter().position(|g| std::ptr::eq(g, f)).unwrap_or(0));
        let _ = writeln!(
            x,
            "            <mc-clip ref=\"m1\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\">",
            esc(scene),
            rt(start, fps),
            rt(prog_tc + start, fps),
            rt(end - start, fps)
        );
        match audio_angle {
            Some(a) => {
                let _ = writeln!(x, "              <mc-source angleID=\"angle{angle}\" srcEnable=\"video\"/>");
                let _ = writeln!(x, "              <mc-source angleID=\"angle{a}\" srcEnable=\"audio\"/>");
            }
            None => {
                let _ = writeln!(x, "              <mc-source angleID=\"angle{angle}\" srcEnable=\"all\"/>");
            }
        }
        // Every file full length on its own lane above the edit (attached to
        // the shot it starts in: the first, but for a file started during
        // the take), switched off like the Premiere timeline's tracks: in
        // parallel and in sync, to swap shots.
        for (i, g) in t.files.iter().enumerate() {
            let at = t.file_start(g);
            if at == start || (at > start && at < end) {
                x += &full_clip(i, &format!(" lane=\"{}\"", i + 1), " enabled=\"0\" srcEnable=\"video\"", "              ");
            }
        }
        for (m, title) in t.markers.iter().filter(|(m, _)| *m >= start && *m < end) {
            let _ = writeln!(x, "              <marker start=\"{}\" duration=\"{}\" value=\"{}\"/>", rt(prog_tc + m, fps), rt(1, fps), esc(title));
        }
        x += "            </mc-clip>\n";
    }
    x += "          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n";
    x
}

fn xm_rate(fps: u32) -> String {
    format!("<rate><timebase>{fps}</timebase><ntsc>FALSE</ntsc></rate>")
}

/// Adobe Premiere Pro (Final Cut Pro 7 XML, which Premiere imports).
pub fn premiere_xml(t: &TakeExport) -> String {
    let mut x = String::new();
    x += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"4\">\n";
    x += "  <sequence id=\"sequence-1\">\n";
    let _ = writeln!(x, "    <name>{}</name>\n    <duration>{}</duration>\n    {}", esc(&t.name), t.frames, xm_rate(t.fps));
    let _ = writeln!(
        x,
        "    <timecode>{}<string>00:00:00:00</string><frame>0</frame><displayformat>NDF</displayformat></timecode>",
        xm_rate(t.fps)
    );
    x += "    <media>\n      <video>\n";
    let _ = writeln!(
        x,
        "        <format><samplecharacteristics>{}<width>{}</width><height>{}</height><anamorphic>FALSE</anamorphic><pixelaspectratio>square</pixelaspectratio><fielddominance>none</fielddominance></samplecharacteristics></format>",
        xm_rate(t.fps),
        t.width,
        t.height
    );
    let mut described: HashSet<usize> = HashSet::new();
    let file_index = |f: &ExportFile| t.files.iter().position(|g| std::ptr::eq(g, f)).unwrap_or(0);
    // (A crop's file is its wide camera's.)
    let file_el = |f: &ExportFile, described: &mut HashSet<usize>| -> String {
        let f = t.media_of(f);
        let i = file_index(f);
        if !described.insert(i) {
            return format!("<file id=\"file-{}\"/>", i + 1);
        }
        let frames = t.file_frames(f);
        let tc = f.tc_frame.unwrap_or(0) as i64;
        let audio = if f.has_audio {
            "<audio><samplecharacteristics><depth>16</depth><samplerate>48000</samplerate></samplecharacteristics><channelcount>2</channelcount></audio>"
        } else {
            ""
        };
        format!(
            "<file id=\"file-{}\"><name>{}</name><pathurl>{}</pathurl>{}<duration>{frames}</duration><timecode>{}<string>{}</string><frame>{tc}</frame><displayformat>NDF</displayformat></timecode><media><video><samplecharacteristics>{}<width>{}</width><height>{}</height></samplecharacteristics></video>{audio}</media></file>",
            i + 1,
            esc(&f.path.file_name().unwrap_or_default().to_string_lossy()),
            esc(&file_url(&f.path, "localhost")),
            xm_rate(f.fps),
            xm_rate(f.fps),
            timecode_string(tc, f.fps),
            xm_rate(f.fps),
            f.width,
            f.height
        )
    };
    let mut clip_id = 0;
    // V1: the live edit.
    x += "        <track>\n";
    let filters = |f: &ExportFile, rect: Option<[f32; 4]>| rect.and_then(|r| t.framing_of(f, r)).map(|fr| fr.premiere_filters((t.width, t.height))).unwrap_or_default();
    for e in t.edit() {
        let f = &t.files[e.file];
        clip_id += 1;
        let _ = writeln!(
            x,
            "          <clipitem id=\"clipitem-{clip_id}\"><name>{}</name><enabled>TRUE</enabled><duration>{}</duration>{}<start>{}</start><end>{}</end><in>{}</in><out>{}</out>{}{}</clipitem>",
            esc(&format!("{} ({})", f.name, e.scene)),
            t.file_frames(t.media_of(f)),
            xm_rate(f.fps),
            e.start,
            e.end,
            e.src_in,
            e.src_out,
            file_el(f, &mut described),
            filters(f, e.rect)
        );
    }
    x += "        </track>\n";
    // Every angle, full length, switched off: drag pieces down to change a shot.
    // (A crop's track: the wide camera's file, framed as at the start.)
    for f in &t.files {
        clip_id += 1;
        let (from, len) = match t.wide_of(f) {
            Some(w) => (t.in_file(w, t.file_start(f)), t.file_frames(f)),
            None => (0, t.file_frames(f)),
        };
        let _ = writeln!(
            x,
            "        <track><enabled>FALSE</enabled><clipitem id=\"clipitem-{clip_id}\"><name>{}</name><enabled>TRUE</enabled><duration>{}</duration>{}<start>{}</start><end>{}</end><in>{from}</in><out>{}</out>{}{}</clipitem></track>",
            esc(&f.name),
            t.file_frames(t.media_of(f)),
            xm_rate(f.fps),
            t.file_start(f),
            t.file_end(f),
            from + len,
            file_el(f, &mut described),
            filters(f, f.crop.as_ref().map(|c| c.rect_at(f.start)))
        );
    }
    x += "      </video>\n      <audio>\n        <numOutputChannels>2</numOutputChannels>\n";
    if let Some(p) = t.sound() {
        for ch in 1..=2 {
            clip_id += 1;
            let _ = writeln!(
                x,
                "        <track><clipitem id=\"clipitem-{clip_id}\"><name>{} audio</name><enabled>TRUE</enabled><duration>{}</duration>{}<start>0</start><end>{}</end><in>0</in><out>{}</out>{}<sourcetrack><mediatype>audio</mediatype><trackindex>{ch}</trackindex></sourcetrack></clipitem></track>",
                esc(&p.name),
                t.file_frames(p),
                xm_rate(p.fps),
                t.frames,
                t.file_frames(p),
                file_el(p, &mut described)
            );
        }
    }
    x += "      </audio>\n    </media>\n";
    for (frame, title) in &t.markers {
        let _ = writeln!(x, "    <marker><comment></comment><name>{}</name><in>{frame}</in><out>-1</out></marker>", esc(title));
    }
    x += "  </sequence>\n</xmeml>\n";
    x
}

/// "1:02:03" or "2:03".
fn timestamp(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// YouTube chapters from markers (seconds, name): the list YouTube reads
/// from a description (Chapters.txt and the upload's description). It needs
/// the first at 0:00, at least three, each at least ten seconds long; None
/// when the markers don't make that.
pub fn youtube_chapters(markers: &[(f64, String)], duration_s: f64) -> Option<String> {
    let mut list: Vec<(f64, String)> = Vec::new();
    for (at, name) in markers.iter().filter(|(at, _)| *at >= 0.0 && *at < duration_s - 10.0) {
        match list.last() {
            Some((prev, _)) if at - prev < 10.0 => {
                // Too close: the later name wins the earlier spot.
                let p = *prev;
                list.pop();
                list.push((p, name.clone()));
            }
            _ => list.push((*at, name.clone())),
        }
    }
    match list.first_mut() {
        Some(first) if first.0 < 10.0 => first.0 = 0.0,
        _ => list.insert(0, (0.0, "Start".into())),
    }
    if list.len() < 3 {
        return None;
    }
    Some(list.iter().map(|(at, name)| format!("{} {}", timestamp(*at as u64), name.trim())).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransitionMode;

    fn ev(at_ms: i64, scene: &str, angle: Option<usize>, mode: TransitionMode) -> SwitchEvent {
        SwitchEvent { at: at_ms * 1_000_000, scene: scene.into(), mode, duration_ms: 400, angle }
    }

    fn sample() -> TakeExport {
        let file = |name: &str, source: Option<usize>, fps| ExportFile {
            path: PathBuf::from(format!("/tmp/Take 1/{name}.mp4")),
            name: name.into(),
            width: 1920,
            height: 1080,
            fps,
            has_audio: true,
            source,
            tc_frame: Some(37230 * fps),
            length: None,
            clean: false,
            start: 0,
            crop: None,
        };
        TakeExport {
            name: "Take & Co".into(),
            fps: 30,
            width: 1920,
            height: 1080,
            frames: 300,
            files: vec![file("Program", None, 30), file("Wide", Some(0), 30), file("Host", Some(1), 25)],
            cuts: vec![
                Cut { frame: 0, angle: Some(0), scene: "Wide".into() },
                Cut { frame: 90, angle: None, scene: "Side by side".into() },
                Cut { frame: 180, angle: Some(1), scene: "Host".into() },
            ],
            markers: vec![(120, "Intro done".into())],
        }
    }

    #[test]
    fn cuts_follow_switches() {
        let recorded: HashSet<usize> = [0, 1].into_iter().collect();
        let events = vec![
            ev(1_050, "Side by side", None, TransitionMode::Cut),
            ev(2_000, "Host", Some(1), TransitionMode::Mix),
            ev(2_010, "Host", Some(1), TransitionMode::Cut), // cuts the mix short
            ev(5_050, "Cam 3", Some(2), TransitionMode::Cut), // not recorded: program
        ];
        let cuts = cuts_from_switches("Wide", Some(0), &events, 0, 50_000_000, 30, 300, &recorded);
        assert_eq!(cuts.iter().map(|c| (c.frame, c.angle)).collect::<Vec<_>>(), vec![(0, Some(0)), (30, None), (59, Some(1)), (150, None)]);
    }

    #[test]
    fn shots_of_several_cameras_use_the_clean_program_when_there_is_one() {
        let mut t = sample();
        let names = |t: &TakeExport| t.shots().iter().map(|s| s.2.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&t), ["Wide", "Program", "Host"]);
        let mut clean = t.files[0].clone();
        clean.name = "Program (clean)".into();
        clean.path = PathBuf::from("/tmp/Take 1/Program_Clean.mp4");
        clean.clean = true;
        t.files.push(clean);
        assert_eq!(names(&t), ["Wide", "Program (clean)", "Host"]);
        assert_eq!(t.program().unwrap().name, "Program", "the sound still comes from the Program file");
        let x = fcpxml(&t);
        assert!(x.contains("Program_Clean.mp4") && x.contains("name=\"Program (clean)\""));
    }

    #[test]
    fn fcpxml_is_well_formed_and_complete() {
        let x = fcpxml(&sample());
        assert!(x.contains("<multicam format=\"f1\" tcStart=\"37230s\""), "{x}");
        // (On Windows "/tmp" is on the current drive: file:///D:/tmp/….)
        assert!(x.contains("src=\"file:///") && x.contains("/tmp/Take%201/Wide.mp4\""), "{x}");
        assert!(x.contains("name=\"Take &amp; Co Multicam\""));
        // Host is 25 fps: its own format and timecode start.
        assert!(x.contains("start=\"37230s\" duration=\"10s\" hasVideo=\"1\" format=\"f2\""), "{x}");
        assert_eq!(x.matches("<mc-clip ").count(), 3);
        assert!(x.contains("<marker start=\"37234s\" duration=\"1/30s\" value=\"Intro done\"/>"), "{x}");
        // Every angle full length on its own lane, in sync with the edit.
        let t = sample();
        assert_eq!(x.matches(" lane=\"").count(), t.files.len(), "{x}");
        for (i, f) in t.files.iter().enumerate() {
            assert!(x.contains(&format!("<asset-clip ref=\"a{}\" lane=\"{}\" name=\"{}\" offset=\"37230s\"", i + 1, i + 1, esc(&f.name))), "{x}");
        }
        let first = &x[x.find("<mc-clip ").unwrap()..x.find("</mc-clip>").unwrap()];
        assert_eq!(first.matches(" lane=\"").count(), t.files.len(), "the lanes hang off the first shot");
        assert_eq!(x.matches('<').count(), x.matches('>').count());
        xml_balanced(&x);
    }

    #[test]
    fn premiere_xml_is_well_formed() {
        let x = premiere_xml(&sample());
        assert!(x.contains("<pathurl>file://localhost/") && x.contains("/tmp/Take%201/Host.mp4</pathurl>"), "{x}");
        // Host at 25 fps: frames 180..300 of the sequence are 150..250 in the file.
        assert!(x.contains("<start>180</start><end>300</end><in>150</in><out>250</out>"), "{x}");
        assert_eq!(x.matches("<file id=\"file-1\">").count(), 1, "each file described once");
        xml_balanced(&x);
    }

    /// Host left the show 8 s in: its file is 8 s long, and its shot (from
    /// 6 s) stops there in Premiere (Final Cut's multicam just runs out of
    /// that angle, with the program's sound going on).
    #[test]
    fn a_source_that_left_early() {
        let mut t = sample();
        t.files[2].length = Some(8 * NS_PER_SEC);
        let x = premiere_xml(&t);
        assert!(x.contains("<start>180</start><end>240</end><in>150</in><out>200</out>"), "{x}");
        assert!(x.contains("<duration>200</duration>"), "{x}");
        let f = fcpxml(&t);
        assert!(f.contains("start=\"37230s\" duration=\"8s\" hasVideo=\"1\" format=\"f2\""), "{f}");
        assert_eq!(f.matches("<mc-clip ").count(), 3);
        xml_balanced(&x);
        xml_balanced(&f);
    }

    /// Host restarted 7 s in: its first file ends there, a second starts
    /// at 7 s. Its shot (from 6 s) takes each part from the file that has
    /// it; the multicam keeps one Host angle with both files in place.
    #[test]
    fn a_source_that_restarted() {
        let mut t = sample();
        t.files[2].length = Some(7 * NS_PER_SEC);
        let mut again = t.files[2].clone();
        again.path = PathBuf::from("/tmp/Take 1/Host (from 00-00-07).mp4");
        again.length = None;
        again.start = 7 * NS_PER_SEC;
        again.tc_frame = Some((37230 + 7) * 25);
        t.files.push(again);
        let e = t.edit();
        let host: Vec<_> = e.iter().filter(|s| s.scene == "Host").map(|s| (s.start, s.end, s.file, s.src_in, s.src_out)).collect();
        // Program frames 180..210 from the first file (its frames 150..175), 210..300 from the second (0..75).
        assert_eq!(host, vec![(180, 210, 2, 150, 175), (210, 300, 3, 0, 75)]);
        assert_eq!((t.file_start(&t.files[3]), t.file_frames(&t.files[3])), (210, 75));
        let x = premiere_xml(&t);
        assert!(x.contains("<start>210</start><end>300</end><in>0</in><out>75</out>"), "{x}");
        xml_balanced(&x);
        let f = fcpxml(&t);
        assert_eq!(f.matches("<mc-angle ").count(), 3, "one Host angle: {f}");
        assert!(f.contains("<asset-clip ref=\"a4\" name=\"Host\" offset=\"37237s\" start=\"37237s\""), "{f}");
        assert!(f.contains("<asset-clip ref=\"a4\" lane=\"4\" name=\"Host\" offset=\"37237s\""), "its lane where it starts: {f}");
        xml_balanced(&f);
        let r = crate::engine::sessions::ResolveEdit::from_export(&t, true);
        assert_eq!((r.angles[3].at, r.angles[3].frames), (210, 75));
    }

    #[test]
    fn file_urls_on_each_system() {
        assert_eq!(file_url_of("/Users/me/Movies/Take 1/Wide.mp4", false, ""), "file:///Users/me/Movies/Take%201/Wide.mp4");
        assert_eq!(file_url_of("/a/b:c.mp4", false, "localhost"), "file://localhost/a/b%3Ac.mp4");
        assert_eq!(file_url_of(r"C:\Users\Me\Videos\Cutaway\Take 1\Wide.mp4", true, ""), "file:///C:/Users/Me/Videos/Cutaway/Take%201/Wide.mp4");
        assert_eq!(file_url_of(r"D:\Show\Host.mp4", true, "localhost"), "file://localhost/D:/Show/Host.mp4");
        assert_eq!(file_url_of(r"\\?\C:\Long\Path.mp4", true, ""), "file:///C:/Long/Path.mp4");
        assert_eq!(file_url_of(r"\\nas\shows\Take 2\Program.mp4", true, "localhost"), "file://nas/shows/Take%202/Program.mp4");
        assert_eq!(file_url_of(r"\\?\UNC\nas\shows\a.mp4", true, ""), "file://nas/shows/a.mp4");
        assert_eq!(file_url_of(r"C:\Takes\10:30.mp4", true, ""), "file:///C:/Takes/10%3A30.mp4");
    }

    #[test]
    fn chapters_follow_youtubes_rules() {
        let m = |v: &[(f64, &str)]| v.iter().map(|(a, n)| (*a, n.to_string())).collect::<Vec<_>>();
        assert_eq!(youtube_chapters(&m(&[(0.0, "Welcome"), (65.0, "News"), (600.0, "Guest")]), 1200.0).unwrap(), "0:00 Welcome\n1:05 News\n10:00 Guest");
        // A first marker after the start gets a "Start" chapter before it.
        assert_eq!(youtube_chapters(&m(&[(30.0, "A"), (90.0, "B")]), 300.0).unwrap(), "0:00 Start\n0:30 A\n1:30 B");
        // Just past the start counts as the start.
        assert!(youtube_chapters(&m(&[(3.0, "A"), (90.0, "B"), (200.0, "C")]), 300.0).unwrap().starts_with("0:00 A"));
        // Too close together: merged; too few: none.
        assert_eq!(youtube_chapters(&m(&[(0.0, "A"), (5.0, "B"), (100.0, "C")]), 300.0), None);
        assert_eq!(youtube_chapters(&m(&[(0.0, "A"), (4000.0, "B"), (5000.0, "C")]), 6000.0).unwrap(), "0:00 A\n1:06:40 B\n1:23:20 C");
        assert_eq!(youtube_chapters(&[], 100.0), None);
    }

    /// A 4K wide camera split in two, in a 1080p take: its left close-up
    /// saved as a crop. Cuts: the wide, the left close-up, the program.
    fn with_crop(keys: Vec<(Ns, [f32; 4])>, shot: (u32, u32)) -> TakeExport {
        let mut t = sample();
        t.files.truncate(2);
        t.files[1].width = 3840;
        t.files[1].height = 2160;
        let mut left = t.files[1].clone();
        left.name = "Wide left".into();
        left.source = Some(5);
        left.has_audio = false;
        left.crop = Some(Crop { of: 0, size: shot, keys });
        t.files.push(left);
        t.cuts = vec![
            Cut { frame: 0, angle: Some(0), scene: "Wide".into() },
            Cut { frame: 90, angle: Some(5), scene: "Wide left".into() },
            Cut { frame: 180, angle: None, scene: "Side by side".into() },
        ];
        t
    }

    const LEFT: [f32; 4] = [0.0, 0.25, 0.5, 0.5];
    const RIGHT: [f32; 4] = [0.5, 0.25, 0.5, 0.5];

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The numbers: zoom over the editor's fit, centre moved in timeline
    /// pixels (right, up), and Final Cut's percent of the frame's height.
    #[test]
    fn crop_framing_numbers() {
        use crate::engine::ptz::{shot_fraction, Shot};
        // 4K wide, halves, in a 1080p timeline: the 1920x1080 half fills it.
        let f = framing(LEFT, (3840, 2160), (1920, 1080), (1920, 1080));
        assert!(close(f.zoom, 2.0) && close(f.x, 960.0) && close(f.y, 0.0) && f.crop.is_none(), "{f:?}");
        assert!(close(f.fit, 0.5) && f.fitted == (1920.0, 1080.0));
        assert_eq!(f.fcp_transform(1080), "position=\"88.8889 0\" scale=\"2 2\"");
        let r = framing(RIGHT, (3840, 2160), (1920, 1080), (1920, 1080));
        assert!(close(r.x, -960.0) && close(r.zoom, 2.0));
        // Thirds (Close-ups… at zoom 3): 1280x720 each.
        let third = |x: f32| framing(shot_fraction(Shot { x, y: 0.5, zoom: 3.0 }, (3840, 2160), (1280, 720)), (3840, 2160), (1280, 720), (1920, 1080));
        let (l, m, r) = (third(1.0 / 6.0), third(0.5), third(5.0 / 6.0));
        assert!(close(l.zoom, 3.0) && (l.x - 1920.0).abs() < 0.01 && (m.x).abs() < 0.01 && (r.x + 1920.0).abs() < 0.01, "{l:?} {m:?} {r:?}");
        assert_eq!(l.fcp_transform(1080), "position=\"177.7778 0\" scale=\"3 3\"");
        // Lower down: the picture moves up.
        let low = framing([0.25, 0.5, 0.5, 0.5], (3840, 2160), (1920, 1080), (1920, 1080));
        assert!(close(low.x, 0.0) && close(low.y, 540.0), "{low:?}");
        assert_eq!(low.fcp_transform(1080), "position=\"0 50\" scale=\"2 2\"");
        // The same shot in a 4K timeline, or from a 1080p wide: the same in Final Cut's terms.
        let k = framing(LEFT, (3840, 2160), (1920, 1080), (3840, 2160));
        assert!(close(k.zoom, 2.0) && close(k.x, 1920.0) && close(k.fit, 1.0));
        assert_eq!(k.fcp_transform(2160), f.fcp_transform(1080));
        let hd = framing(LEFT, (1920, 1080), (960, 540), (1920, 1080));
        assert_eq!(hd.fcp_transform(1080), f.fcp_transform(1080));
        // Premiere: percent of the file's own pixels (4K at 100% fills
        // 1080p at twice the fit), centre a fraction of the frame.
        assert!(f.premiere_filters((1920, 1080)).contains("<value>100</value>") && f.premiere_filters((1920, 1080)).contains("<horiz>0.5</horiz><vert>0</vert>"));
        assert!(hd.premiere_filters((1920, 1080)).contains("<value>200</value>"));
        // A tall close-up (9:16) of a 1080p camera in a 1080p timeline: the
        // middle slice, trimmed to its shape, with bars as its own file has.
        let tall = framing(shot_fraction(Shot::default(), (1920, 1080), (1080, 1920)), (1920, 1080), (1080, 1920), (1920, 1080));
        let c = tall.crop.expect("trimmed");
        assert!(close(tall.zoom, 1.0) && close(tall.x, 0.0) && (c[0] - 656.25).abs() < 0.01 && (c[1] - 656.25).abs() < 0.01 && c[2] == 0.0 && c[3] == 0.0, "{tall:?}");
        assert_eq!(tall.fcp_trim().unwrap(), "left=\"60.7639\" top=\"0\" right=\"60.7639\" bottom=\"0\"");
        assert!(tall.premiere_filters((1920, 1080)).contains("<parameterid>left</parameterid><name>left</name><valuemin>0</valuemin><valuemax>100</valuemax><value>34.1797</value>"));
        assert_eq!(f.fcp_trim(), None, "a shot the timeline's shape needs no trim");
    }

    #[test]
    fn moves_become_steady_framings() {
        let ms = |m: i64| m * 1_000_000;
        // Rest on the left, a glide (5 a second) to the right, rest.
        let mid = [0.25, 0.25, 0.5, 0.5];
        let keys = vec![(0, LEFT), (ms(4000), LEFT), (ms(4033), [0.1, 0.25, 0.5, 0.5]), (ms(4233), mid), (ms(4433), [0.4, 0.25, 0.5, 0.5]), (ms(4600), RIGHT)];
        assert_eq!(crop_steps(&keys), vec![(0, LEFT), (ms(4000), RIGHT)]);
        assert_eq!(crop_steps(&keys[..1]), vec![(0, LEFT)]);
        assert!(crop_steps(&[]).is_empty());
        let c = Crop { of: 0, size: (1920, 1080), keys };
        assert_eq!(c.rect_at(ms(4300)), mid);
        assert_eq!(c.rect_at(-5), LEFT);
    }

    /// The crop's angle is the wide camera's file, framed: no file or asset
    /// of its own, its shot in the multicam angle and its lane.
    #[test]
    fn fcpxml_frames_a_crop_from_the_wide_file() {
        let t = with_crop(vec![(0, LEFT)], (1920, 1080));
        let x = fcpxml(&t);
        xml_balanced(&x);
        assert_eq!(x.matches("<asset id=").count(), 2, "no asset of its own: {x}");
        assert_eq!(x.matches("<mc-angle ").count(), 3, "{x}");
        let angle = &x[x.find("<mc-angle name=\"Wide left\"").unwrap()..];
        let angle = &angle[..angle.find("</mc-angle>").unwrap()];
        assert!(angle.contains("<asset-clip ref=\"a2\" name=\"Wide left\" offset=\"37230s\" start=\"37230s\" duration=\"10s\""), "{angle}");
        assert!(angle.contains("<adjust-transform position=\"88.8889 0\" scale=\"2 2\"/>"), "{angle}");
        assert!(!angle.contains("adjust-crop") && !angle.contains("keyframe"), "{angle}");
        assert!(x.contains("<asset-clip ref=\"a2\" lane=\"3\" name=\"Wide left\""), "{x}");
        assert_eq!(x.matches("<adjust-transform ").count(), 2, "the angle and its lane: {x}");
        // The edit cuts to it like any angle.
        assert!(x.contains("<mc-source angleID=\"angle2\" srcEnable=\"video\"/>"), "{x}");
        // The edit's other formats: the wide file, framed.
        let e = t.edit();
        assert_eq!(e[1].file, 2);
        assert_eq!((e[1].start, e[1].end, e[1].rect), (90, 180, Some(LEFT)));
        assert_eq!(e[0].rect, None);
        let p = premiere_xml(&t);
        xml_balanced(&p);
        assert!(!p.contains("file-3") && p.matches("<file id=\"file-2\">").count() == 1, "the crop is the wide camera's file: {p}");
        assert!(p.contains("<name>Wide left (Wide left)</name>") && p.contains("<horiz>0.5</horiz>"), "{p}");
        assert_eq!(p.matches("Basic Motion").count(), 2, "its V1 shot and its track: {p}");
        let r = crate::engine::sessions::ResolveEdit::from_export(&t, true);
        assert_eq!(r.cuts[1].file, "Wide.mp4");
        assert!(r.cuts[1].framing.is_some_and(|f| close(f.zoom, 2.0) && close(f.x, 960.0)));
        assert!(r.cuts[0].framing.is_none() && r.angles[2].framing.is_some());
    }

    /// It moved during its shot: Final Cut gets keyframes on the angle;
    /// the others cut to where it went.
    #[test]
    fn a_crop_that_moved() {
        let ms = |m: i64| m * 1_000_000;
        let keys = vec![(0, LEFT), (ms(4000), LEFT), (ms(4200), [0.25, 0.25, 0.5, 0.5]), (ms(4400), RIGHT)];
        let t = with_crop(keys, (1920, 1080));
        let x = fcpxml(&t);
        xml_balanced(&x);
        let angle = &x[x.find("<mc-angle name=\"Wide left\"").unwrap()..];
        let angle = &angle[..angle.find("</mc-angle>").unwrap()];
        // In the clip's own time: its timecode plus the time into the file.
        assert!(angle.contains("<keyframe time=\"37230s\" value=\"88.8889 0\"/>"), "{angle}");
        assert!(angle.contains("<keyframe time=\"37234s\" value=\"88.8889 0\"/>"), "{angle}");
        assert!(angle.contains("<keyframe time=\"186171/5s\" value=\"0 0\"/>"), "{angle}");
        assert!(angle.contains("<keyframe time=\"186172/5s\" value=\"-88.8889 0\"/>"), "{angle}");
        assert_eq!(angle.matches("<keyframe ").count(), 8, "position and scale: {angle}");
        // Its shot (frames 90..180, 3 to 6 s): left till 4 s, then right.
        let e: Vec<_> = t.edit().into_iter().filter(|e| e.file == 2).map(|e| (e.start, e.end, e.rect)).collect();
        assert_eq!(e, vec![(90, 120, Some(LEFT)), (120, 180, Some(RIGHT))]);
        let p = premiere_xml(&t);
        xml_balanced(&p);
        assert!(p.contains("<start>90</start><end>120</end><in>90</in><out>120</out>") && p.contains("<start>120</start><end>180</end><in>120</in><out>180</out>"), "{p}");
    }

    /// A tall close-up: the trim goes with each steady framing.
    #[test]
    fn a_crop_of_another_shape_is_trimmed() {
        let tall = crate::engine::ptz::shot_fraction(crate::engine::ptz::Shot::default(), (3840, 2160), (1080, 1920));
        let t = with_crop(vec![(0, tall)], (1080, 1920));
        let x = fcpxml(&t);
        xml_balanced(&x);
        assert!(x.contains("<adjust-crop mode=\"trim\">") && x.contains("<trim-rect left=\"60.7639\" top=\"0\" right=\"60.7639\" bottom=\"0\"/>"), "{x}");
        // Trim before transform, as Final Cut's DTD orders them.
        assert!(x.find("<adjust-crop").unwrap() < x.find("<adjust-transform").unwrap());
        let p = premiere_xml(&t);
        assert!(p.contains("<name>Crop</name>"), "{p}");
    }

    /// A close-up that joined later starts part way into the wide file;
    /// one whose wide camera left ends with it.
    #[test]
    fn a_crop_follows_its_files() {
        let mut t = with_crop(vec![(2 * NS_PER_SEC, LEFT)], (1920, 1080));
        t.files[2].start = 2 * NS_PER_SEC;
        t.files[1].length = Some(8 * NS_PER_SEC);
        assert_eq!((t.file_start(&t.files[2]), t.file_frames(&t.files[2])), (60, 180));
        let x = fcpxml(&t);
        assert!(x.contains("<asset-clip ref=\"a2\" name=\"Wide left\" offset=\"37232s\" start=\"37232s\" duration=\"6s\""), "{x}");
        let e: Vec<_> = t.edit().into_iter().filter(|e| e.file == 2).map(|e| (e.start, e.end, e.src_in, e.src_out)).collect();
        assert_eq!(e, vec![(90, 180, 90, 180)], "in the wide file's own frames");
        let r = crate::engine::sessions::ResolveEdit::from_export(&t, true);
        assert_eq!((r.angles[2].at, r.angles[2].from, r.angles[2].frames), (60, 60, 180));
        let p = premiere_xml(&t);
        assert!(p.contains("<start>60</start><end>240</end><in>60</in><out>240</out>"), "{p}");
    }

    /// Minimal check that tags open and close in order.
    fn xml_balanced(x: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = x;
        while let Some(i) = rest.find('<') {
            let end = rest[i..].find('>').unwrap() + i;
            let tag = &rest[i + 1..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
                continue;
            }
            let name = tag.split_whitespace().next().unwrap();
            if let Some(closing) = name.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(closing), "mismatched </{closing}>");
            } else {
                stack.push(name.to_string());
            }
        }
        assert!(stack.is_empty(), "unclosed: {stack:?}");
    }
}
