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

use super::switcher::SwitchEvent;
use crate::media::{Ns, NS_PER_SEC};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// Program frame (from the start of the take) where this shot begins.
    pub frame: i64,
    /// Source index, or None for the program.
    pub angle: Option<usize>,
    pub scene: String,
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
        self.files.iter().find(|f| f.source == angle).or_else(|| self.program())
    }

    /// Length of a file in its own frames (every file is the take length).
    fn file_frames(&self, f: &ExportFile) -> i64 {
        (self.frames * f.fps as i64 + self.fps as i64 / 2) / self.fps as i64
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

    let mut x = String::new();
    x += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.9\">\n  <resources>\n";
    for (w, h, r) in &formats {
        let _ = writeln!(x, "    <format id=\"{}\" frameDuration=\"{}\" width=\"{w}\" height=\"{h}\" colorSpace=\"1-1-1 (Rec. 709)\"/>", fmt_id(*w, *h, *r), rt(1, *r as i64));
    }
    for (i, f) in t.files.iter().enumerate() {
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
    let _ = writeln!(x, "    <media id=\"m1\" name=\"{} Multicam\">", esc(&t.name));
    let _ = writeln!(x, "      <multicam format=\"{}\" tcStart=\"{}\" tcFormat=\"NDF\">", fmt_id(t.width, t.height, t.fps), rt(mc_start.0, mc_start.1));
    for (i, f) in t.files.iter().enumerate() {
        let (sn, sd) = asset_start(f);
        let _ = writeln!(x, "        <mc-angle name=\"{}\" angleID=\"angle{i}\">", esc(&f.name));
        let _ = writeln!(
            x,
            "          <asset-clip ref=\"a{}\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" format=\"{}\" tcFormat=\"NDF\"/>",
            i + 1,
            esc(&f.name),
            rt(mc_start.0, mc_start.1),
            rt(sn, sd),
            rt(t.file_frames(f), f.fps as i64),
            fmt_id(f.width, f.height, f.fps)
        );
        x += "        </mc-angle>\n";
    }
    x += "      </multicam>\n    </media>\n  </resources>\n";
    let _ = writeln!(x, "  <library>\n    <event name=\"Cutaway\">\n      <project name=\"{}\">", esc(&t.name));
    let _ = writeln!(
        x,
        "        <sequence format=\"{}\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\" audioLayout=\"stereo\" audioRate=\"48k\">\n          <spine>",
        fmt_id(t.width, t.height, t.fps),
        rt(t.frames, fps)
    );
    let audio_angle = t.files.iter().position(|f| f.source.is_none() && f.has_audio).or_else(|| t.files.iter().position(|f| f.has_audio));
    for (start, end, f, scene) in t.shots() {
        let angle = t.files.iter().position(|g| std::ptr::eq(g, f)).unwrap_or(0);
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
    let file_el = |f: &ExportFile, described: &mut HashSet<usize>| -> String {
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
    let in_file = |f: &ExportFile, seq_frame: i64| (seq_frame * f.fps as i64 + t.fps as i64 / 2) / t.fps as i64;
    let mut clip_id = 0;
    // V1: the live edit.
    x += "        <track>\n";
    for (start, end, f, scene) in t.shots() {
        clip_id += 1;
        let _ = writeln!(
            x,
            "          <clipitem id=\"clipitem-{clip_id}\"><name>{}</name><enabled>TRUE</enabled><duration>{}</duration>{}<start>{start}</start><end>{end}</end><in>{}</in><out>{}</out>{}</clipitem>",
            esc(&format!("{} ({})", f.name, scene)),
            t.file_frames(f),
            xm_rate(f.fps),
            in_file(f, start),
            in_file(f, end),
            file_el(f, &mut described)
        );
    }
    x += "        </track>\n";
    // Every angle, full length, switched off: drag pieces down to change a shot.
    for f in &t.files {
        clip_id += 1;
        let _ = writeln!(
            x,
            "        <track><enabled>FALSE</enabled><clipitem id=\"clipitem-{clip_id}\"><name>{}</name><enabled>TRUE</enabled><duration>{}</duration>{}<start>0</start><end>{}</end><in>0</in><out>{}</out>{}</clipitem></track>",
            esc(&f.name),
            t.file_frames(f),
            xm_rate(f.fps),
            t.frames,
            t.file_frames(f),
            file_el(f, &mut described)
        );
    }
    x += "      </video>\n      <audio>\n        <numOutputChannels>2</numOutputChannels>\n";
    if let Some(p) = t.files.iter().find(|f| f.source.is_none() && f.has_audio).or_else(|| t.files.iter().find(|f| f.has_audio)) {
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

/// YouTube chapter list ("0:00 Start" first, as YouTube requires).
pub fn youtube_chapters(markers: &[(Ns, String)]) -> String {
    let fmt = |ns: Ns| {
        let s = (ns.max(0) / NS_PER_SEC) as u64;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    };
    let mut out = String::new();
    if markers.first().is_none_or(|m| m.0 >= NS_PER_SEC) {
        out += "0:00 Start\n";
    }
    for (t, title) in markers {
        let _ = writeln!(out, "{} {}", fmt(*t), title);
    }
    out
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
    fn chapters_list() {
        let c = youtube_chapters(&[(65 * NS_PER_SEC, "Guest".into()), (3725 * NS_PER_SEC, "Outro".into())]);
        assert_eq!(c, "0:00 Start\n1:05 Guest\n1:02:05 Outro\n");
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
