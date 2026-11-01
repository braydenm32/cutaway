//! Hands a take to audio and video software as a ready-made project:
//!
//! * `<take>.rpp` for REAPER: every WAV on its own track, lined up, with
//!   the program video on top and the markers.
//! * `<take>.dawproject` for Fender Studio Pro (Studio One), Bitwig and
//!   Cubase: the open DAWproject format, referring to the WAVs next to it.
//! * `<take> (Resolve).lua`: what Cutaway's DaVinci Resolve script reads to
//!   build a project (bins, media, proxies, the live edit). The script
//!   itself is installed once from Record › Recording.
//!
//! Every file starts at the same instant, so every track starts at zero.

use crate::media::wav::WavReader;
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One audio file in the session.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    /// File name (next to the session file).
    pub file: String,
    pub channels: u16,
    pub rate: u32,
    pub seconds: f64,
    /// Starts muted (the mix, which repeats everything else).
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub name: String,
    pub tracks: Vec<Track>,
    /// Program video file name, if there is one.
    pub video: Option<(String, f64)>,
    /// (seconds from the start, name)
    pub markers: Vec<(f64, String)>,
    /// Time of day at the start (seconds), for the timeline's clock.
    pub start_tc: Option<f64>,
    pub fps: u32,
}

/// "Audio_02_Guest_ch3.wav" -> "Guest 3"; "Mix.wav" -> "Mix".
fn track_name(file: &str) -> String {
    let stem = file.trim_end_matches(".wav");
    let rest = stem.strip_prefix("Audio_").map(|r| r.split_once('_').map(|(_, n)| n).unwrap_or(r)).unwrap_or(stem);
    match rest.rsplit_once("_ch") {
        Some((n, c)) if c.chars().all(|x| x.is_ascii_digit()) && !c.is_empty() => format!("{} {c}", n.replace('_', " ")),
        _ => rest.replace('_', " "),
    }
}

impl Session {
    /// From a finished take folder: its WAVs (inputs, then the mix) and
    /// program video.
    pub fn from_folder(folder: &Path, markers: &[(f64, String)], start_tc: Option<f64>, fps: u32) -> Result<Session> {
        let name = folder.file_name().unwrap_or_default().to_string_lossy().to_string();
        let mut wavs: Vec<String> = std::fs::read_dir(folder)
            .with_context(|| format!("reading {}", folder.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.to_lowercase().ends_with(".wav"))
            .collect();
        // Inputs in order (Audio_01…, channel order), the mix last.
        wavs.sort_by_key(|n| (n == "Mix.wav", natural_key(n)));
        let mut tracks = Vec::new();
        for f in wavs {
            let Ok(r) = WavReader::open(&folder.join(&f)) else { continue };
            let seconds = r.frames as f64 / r.rate.max(1) as f64;
            tracks.push(Track { name: track_name(&f), muted: f == "Mix.wav", file: f, channels: r.channels, rate: r.rate, seconds });
        }
        let video = ["Program.mp4", "Program.mov"].iter().find(|v| folder.join(v).exists()).map(|v| {
            let secs = tracks.iter().map(|t| t.seconds).fold(0.0, f64::max);
            (v.to_string(), secs)
        });
        Ok(Session { name, tracks, video, markers: markers.to_vec(), start_tc, fps })
    }

    /// A REAPER project (.rpp).
    pub fn reaper(&self) -> String {
        let q = |s: &str| s.replace('"', "'");
        let mut s = String::from("<REAPER_PROJECT 0.1 \"7.0\" 0\n");
        let rate = self.tracks.first().map(|t| t.rate).unwrap_or(48_000);
        let _ = writeln!(s, "  SAMPLERATE {rate} 0 0");
        let _ = writeln!(s, "  PROJOFFS {:.6} 0 0", self.start_tc.unwrap_or(0.0));
        let _ = writeln!(s, "  TIMEMODE 5 5 -1 30 0 0 -1");
        for (i, (t, n)) in self.markers.iter().enumerate() {
            let _ = writeln!(s, "  MARKER {} {t:.6} \"{}\" 0 0 1", i + 1, q(n));
        }
        let item = |s: &mut String, name: &str, file: &str, len: f64, kind: &str, channels: u16, muted: bool| {
            let _ = writeln!(s, "  <TRACK");
            let _ = writeln!(s, "    NAME \"{}\"", q(name));
            let _ = writeln!(s, "    NCHAN {}", (channels.max(2) as u32).div_ceil(2) * 2);
            let _ = writeln!(s, "    MUTESOLO {} 0 0", if muted { 1 } else { 0 });
            let _ = writeln!(s, "    <ITEM");
            let _ = writeln!(s, "      POSITION 0");
            let _ = writeln!(s, "      LENGTH {len:.6}");
            let _ = writeln!(s, "      NAME \"{}\"", q(file));
            let _ = writeln!(s, "      <SOURCE {kind}");
            let _ = writeln!(s, "        FILE \"{}\"", q(file));
            let _ = writeln!(s, "      >");
            let _ = writeln!(s, "    >");
            let _ = writeln!(s, "  >");
        };
        if let Some((v, len)) = &self.video {
            item(&mut s, "Program (video)", v, *len, "VIDEO", 2, true);
        }
        for t in &self.tracks {
            item(&mut s, &t.name, &t.file, t.seconds, "WAVE", t.channels, t.muted);
        }
        s.push_str(">\n");
        s
    }

    /// The DAWproject's project.xml.
    pub fn dawproject_xml(&self) -> String {
        let e = xml_escape;
        let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Project version=\"1.0\">\n");
        let _ = writeln!(s, "  <Application name=\"Cutaway\" version=\"{}\"/>", env!("CARGO_PKG_VERSION"));
        s.push_str("  <Transport>\n    <Tempo unit=\"bpm\" value=\"120\" id=\"tempo\" name=\"Tempo\"/>\n    <TimeSignature numerator=\"4\" denominator=\"4\" id=\"timesig\"/>\n  </Transport>\n  <Structure>\n");
        for (i, t) in self.tracks.iter().enumerate() {
            let _ = writeln!(s, "    <Track contentType=\"audio\" loaded=\"true\" id=\"track{i}\" name=\"{}\">", e(&t.name));
            let _ = writeln!(s, "      <Channel audioChannels=\"{}\" destination=\"master\" role=\"regular\" solo=\"false\" id=\"channel{i}\">", t.channels);
            let _ = writeln!(s, "        <Mute value=\"{}\" id=\"mute{i}\" name=\"Mute\"/>", t.muted);
            let _ = writeln!(s, "        <Pan unit=\"normalized\" value=\"0.5\" id=\"pan{i}\" name=\"Pan\"/>");
            let _ = writeln!(s, "        <Volume unit=\"linear\" value=\"1.0\" id=\"volume{i}\" name=\"Volume\"/>");
            s.push_str("      </Channel>\n    </Track>\n");
        }
        s.push_str("    <Track contentType=\"audio\" loaded=\"true\" id=\"mastertrack\" name=\"Master\">\n      <Channel audioChannels=\"2\" role=\"master\" solo=\"false\" id=\"master\">\n        <Volume unit=\"linear\" value=\"1.0\" id=\"mastervolume\" name=\"Volume\"/>\n      </Channel>\n    </Track>\n  </Structure>\n");
        s.push_str("  <Arrangement id=\"arrangement\">\n    <Lanes timeUnit=\"seconds\" id=\"lanes\">\n");
        for (i, t) in self.tracks.iter().enumerate() {
            let len = format!("{:.6}", t.seconds);
            let _ = writeln!(s, "      <Lanes track=\"track{i}\" id=\"lanes{i}\">");
            let _ = writeln!(s, "        <Clips id=\"clips{i}\">");
            let _ = writeln!(s, "          <Clip time=\"0\" duration=\"{len}\" contentTimeUnit=\"seconds\" playStart=\"0\" playStop=\"{len}\" name=\"{}\">", e(&t.name));
            let _ = writeln!(s, "            <Audio channels=\"{}\" sampleRate=\"{}\" duration=\"{len}\" id=\"audio{i}\">", t.channels, t.rate);
            let _ = writeln!(s, "              <File path=\"{}\" external=\"true\"/>", e(&t.file));
            s.push_str("            </Audio>\n          </Clip>\n        </Clips>\n      </Lanes>\n");
        }
        s.push_str("    </Lanes>\n");
        if !self.markers.is_empty() {
            s.push_str("    <Markers id=\"markers\">\n");
            for (t, n) in &self.markers {
                let _ = writeln!(s, "      <Marker time=\"{t:.6}\" name=\"{}\"/>", e(n));
            }
            s.push_str("    </Markers>\n");
        }
        s.push_str("  </Arrangement>\n</Project>\n");
        s
    }

    pub fn dawproject_metadata(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<MetaData>\n  <Title>{}</Title>\n  <Comment>Recorded with Cutaway. The audio files are next to this project.</Comment>\n</MetaData>\n",
            xml_escape(&self.name)
        )
    }

    /// Write `<name>.dawproject` (a zip of the two XML files).
    pub fn write_dawproject(&self, path: &Path) -> Result<()> {
        let zip = crate::util::zip_stored(&[("project.xml", self.dawproject_xml().as_bytes()), ("metadata.xml", self.dawproject_metadata().as_bytes())]);
        std::fs::write(path, zip).with_context(|| format!("writing {}", path.display()))
    }

    /// What the Resolve script reads: the take as a Lua table. (The paths
    /// are where the take is now; the script finds them again if it's moved.)
    pub fn resolve_lua(&self, folder: &Path, cameras: &[String], timeline: Option<&str>, canvas: (u32, u32), edit: &ResolveEdit) -> String {
        let lua = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " "));
        let path = |f: &str| lua(&folder.join(f).to_string_lossy());
        let mut s = String::from("-- Written by Cutaway: read by the \"Cutaway - Build project from a take\" script in DaVinci Resolve.\nreturn {\n");
        let _ = writeln!(s, "  name = {},", lua(&self.name));
        let _ = writeln!(s, "  fps = {},", self.fps);
        let _ = writeln!(s, "  width = {},", canvas.0);
        let _ = writeln!(s, "  height = {},", canvas.1);
        let _ = writeln!(s, "  folder = {},", lua(&folder.to_string_lossy()));
        s.push_str("  cameras = {");
        for c in cameras {
            let _ = write!(s, " {},", path(c));
        }
        s.push_str(" },\n  proxies = {");
        for c in cameras {
            let p = folder.join("Proxies").join(c);
            if p.exists() {
                let _ = write!(s, " [{}] = {},", path(c), lua(&p.to_string_lossy()));
            }
        }
        s.push_str(" },\n  audio = {");
        for t in &self.tracks {
            let _ = write!(s, " {},", path(&t.file));
        }
        s.push_str(" },\n");
        match timeline {
            Some(t) => {
                let _ = writeln!(s, "  timeline = {},", path(t));
            }
            None => s.push_str("  timeline = nil,\n"),
        }
        s.push_str("  markers = {");
        for (t, n) in &self.markers {
            let _ = write!(s, " {{ frame = {}, name = {} }},", (t * self.fps as f64).round() as i64, lua(n));
        }
        s.push_str(" },\n");
        // The live edit, shot by shot, for the script to lay down itself
        // (the same cuts as the .fcpxml and Premiere timelines).
        if !edit.cuts.is_empty() {
            let _ = writeln!(s, "  frames = {},", edit.frames);
            // The sound under the edit: Mix.wav, or the program's own.
            let sound = edit.sound.clone().or_else(|| self.tracks.iter().find(|t| t.file == "Mix.wav").map(|t| t.file.clone()));
            if let Some(f) = sound {
                let _ = writeln!(s, "  sound = {},", path(&f));
            }
            s.push_str("  edit = {\n");
            for c in &edit.cuts {
                let _ = writeln!(s, "    {{ file = {}, scene = {}, at = {}, length = {}, from = {}, to = {} }},", path(&c.file), lua(&c.scene), c.at, c.length, c.from, c.to);
            }
            s.push_str("  },\n");
            // Every angle full length, each on its own track above the edit.
            s.push_str("  angles = {\n");
            for a in &edit.angles {
                let _ = writeln!(s, "    {{ file = {}, name = {}, frames = {} }},", path(&a.file), lua(&a.name), a.frames);
            }
            s.push_str("  },\n");
        }
        s.push_str("}\n");
        s
    }
}

/// One shot of the live edit for the Resolve script: from program frame
/// `at`, `length` frames of `file` (a name in the take's folder), its own
/// frames `from..to`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolveCut {
    pub file: String,
    pub scene: String,
    pub at: i64,
    pub length: i64,
    pub from: i64,
    pub to: i64,
}

/// One recorded angle (a camera, or the program), whole, in its own frames.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolveAngle {
    pub file: String,
    pub name: String,
    pub frames: i64,
}

/// The live edit the Resolve script lays down (empty: it imports the .fcpxml).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolveEdit {
    pub cuts: Vec<ResolveCut>,
    /// Every angle, for the tracks above the edit.
    pub angles: Vec<ResolveAngle>,
    /// The sound under the edit (a file name), when not Mix.wav.
    pub sound: Option<String>,
    /// The edit's length in program frames.
    pub frames: i64,
}

impl ResolveEdit {
    /// From the take's export: the same shots the Premiere timeline has.
    pub fn from_export(te: &super::export::TakeExport, have_mix: bool) -> ResolveEdit {
        let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().to_string();
        let cuts = te
            .edit()
            .into_iter()
            .map(|e| ResolveCut { file: name(&te.files[e.file].path), scene: e.scene, at: e.start, length: e.end - e.start, from: e.src_in, to: e.src_out })
            .collect();
        let angles = te.files.iter().map(|f| ResolveAngle { file: name(&f.path), name: f.name.clone(), frames: te.file_frames(f) }).collect();
        let sound = if have_mix { None } else { te.sound().map(|f| name(&f.path)) };
        ResolveEdit { cuts, angles, sound, frames: te.frames }
    }
}

/// What a take's Premiere timeline says (written by `export::premiere_xml`):
/// for takes whose Resolve take file has no cut list (older ones, or
/// takes recorded without "a project from each take").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PremiereTake {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub edit: ResolveEdit,
    /// (program frame, name)
    pub markers: Vec<(i64, String)>,
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text of the first `<tag>…</tag>` in `x`.
fn tag<'a>(x: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let a = x.find(&open)? + open.len();
    let b = x[a..].find(&close)? + a;
    Some(&x[a..b])
}

/// Reads the live edit back from Cutaway's own Premiere XML: V1's clips
/// (and their files' names) and the sequence markers.
pub fn premiere_take(xml: &str) -> Option<PremiereTake> {
    let num = |x: &str, t: &str| tag(x, t).and_then(|v| v.trim().parse::<i64>().ok());
    // The sequence's video (files describe their own <video> inside it).
    let v = xml.find("<video>")? + 7;
    let video = &xml[v..xml.find("</video>\n      <audio>").or_else(|| xml.rfind("</video>")).filter(|e| *e > v)?];
    let fps = num(xml, "timebase")? as u32;
    let width = num(video, "width").unwrap_or(1920) as u32;
    let height = num(video, "height").unwrap_or(1080) as u32;
    let frames = num(xml, "duration").unwrap_or(0);
    // Every file's name by id (each is described once, then referred to).
    let mut names = std::collections::HashMap::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<file id=\"") {
        rest = &rest[i + 10..];
        let Some(q) = rest.find('"') else { break };
        let id = rest[..q].to_string();
        if rest[q..].starts_with("\">") {
            if let Some(n) = tag(rest, "name") {
                names.entry(id).or_insert_with(|| xml_unescape(n));
            }
        }
    }
    // V1 is the first track of the video; every angle follows, one a track.
    let mut tracks = video.split("<track>").skip(1).map(|t| t.split("</track>").next().unwrap_or(""));
    let v1 = tracks.next()?;
    let file_of = |item: &str| item.find("<file id=\"").map(|i| &item[i + 10..]).and_then(|r| r.split('"').next()).and_then(|id| names.get(id)).cloned();
    let angles = tracks
        .filter_map(|t| {
            let item = t.split("<clipitem ").nth(1)?;
            Some(ResolveAngle { file: file_of(item)?, name: xml_unescape(tag(item, "name")?), frames: num(item, "out")? })
        })
        .collect();
    let mut cuts = Vec::new();
    for item in v1.split("<clipitem ").skip(1) {
        let (Some(start), Some(end), Some(from), Some(to)) = (num(item, "start"), num(item, "end"), num(item, "in"), num(item, "out")) else { continue };
        let Some(file) = file_of(item) else { continue };
        // "Cam middle (Cam middle)": the angle, then the scene.
        let label = xml_unescape(tag(item, "name").unwrap_or(""));
        let scene = label.rsplit_once(" (").map(|(_, sc)| sc.trim_end_matches(')').to_string()).unwrap_or(label);
        cuts.push(ResolveCut { file, scene, at: start, length: end - start, from, to });
    }
    let mut markers = Vec::new();
    let after = xml.rfind("</media>").map(|i| &xml[i..]).unwrap_or("");
    for m in after.split("<marker>").skip(1) {
        if let (Some(n), Some(at)) = (tag(m, "name"), num(m, "in")) {
            markers.push((at, xml_unescape(n)));
        }
    }
    Some(PremiereTake { fps, width, height, edit: ResolveEdit { cuts, angles, sound: None, frames }, markers })
}


/// Sort key that puts "ch2" before "ch10".
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
            }
            text.push(c);
        }
    }
    out.push((text, num.parse().unwrap_or(0)));
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The DaVinci Resolve script (Workspace → Scripts → Utility).
pub const RESOLVE_SCRIPT_NAME: &str = "Cutaway - Build project from a take.lua";
pub const RESOLVE_SCRIPT: &str = include_str!("resolve_script.lua");

/// Where Resolve looks for this user's scripts.
pub fn resolve_scripts_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        crate::config::home_dir().join("Library/Application Support/Blackmagic Design/DaVinci Resolve/Fusion/Scripts/Utility")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(crate::config::home_dir).join("Blackmagic Design\\DaVinci Resolve\\Support\\Fusion\\Scripts\\Utility")
    } else {
        crate::config::home_dir().join(".local/share/DaVinciResolve/Fusion/Scripts/Utility")
    }
}

/// Install (or update) the script for Resolve. Returns where it went.
pub fn install_resolve_script() -> Result<PathBuf> {
    let dir = resolve_scripts_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(RESOLVE_SCRIPT_NAME);
    std::fs::write(&path, RESOLVE_SCRIPT).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The places DaVinci Resolve (free or Studio) installs itself, most likely
/// first: the app on a Mac, Resolve.exe on Windows, the binary on Linux.
pub fn resolve_app_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        let home = crate::config::home_dir();
        for root in [PathBuf::from("/Applications"), home.join("Applications")] {
            for (dir, app) in [("DaVinci Resolve", "DaVinci Resolve.app"), ("DaVinci Resolve Studio", "DaVinci Resolve Studio.app"), ("DaVinci Resolve", "DaVinci Resolve Studio.app")] {
                out.push(root.join(dir).join(app));
            }
            out.push(root.join("DaVinci Resolve.app"));
        }
    } else if cfg!(windows) {
        // Program Files (64-bit Resolve, whatever the process is).
        let mut roots: Vec<PathBuf> = ["ProgramW6432", "ProgramFiles"].iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
        roots.push(PathBuf::from("C:\\Program Files"));
        roots.dedup();
        for r in roots {
            out.push(r.join("Blackmagic Design").join("DaVinci Resolve").join("Resolve.exe"));
        }
    } else {
        out.push(PathBuf::from("/opt/resolve/bin/resolve"));
    }
    out
}

/// Where DaVinci Resolve is installed, if it's in one of the usual places.
pub fn find_resolve() -> Option<PathBuf> {
    resolve_app_candidates().into_iter().find(|p| p.exists())
}

/// The take's live-edit timeline for Resolve (`<take>.fcpxml`), if it was saved.
pub fn take_timeline(folder: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fcpxml"))).collect();
    found.sort();
    found.into_iter().next()
}

/// Start DaVinci Resolve, handing it `file` (the take's timeline) when
/// there is one. Fails when Resolve isn't installed (or won't start).
pub fn open_in_resolve(file: Option<&Path>) -> Result<()> {
    use std::process::Command;
    let app = find_resolve();
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        match &app {
            Some(a) => cmd.arg("-a").arg(a),
            // Somewhere else: Launch Services knows it by name (and says
            // straight away when it doesn't).
            None => cmd.args(["-a", "DaVinci Resolve"]),
        };
        if let Some(f) = file {
            cmd.arg(f);
        }
        if app.is_some() {
            cmd.spawn().context("starting DaVinci Resolve")?;
        } else {
            let ok = cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
            if !ok {
                anyhow::bail!("DaVinci Resolve isn't installed");
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let Some(exe) = app else { anyhow::bail!("DaVinci Resolve isn't installed") };
        let mut cmd = Command::new(&exe);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        if let Some(f) = file {
            cmd.arg(f);
        }
        cmd.spawn().context("starting DaVinci Resolve")?;
        Ok(())
    }
}

/// Where Cutaway notes the latest take, for the Resolve script's default.
pub fn last_take_file(config_dir: &Path) -> PathBuf {
    config_dir.join("last-take.txt")
}

/// Open in Resolve's note to the script, next to last-take.txt: "build
/// this take now" (its folder, then when, in seconds since 1970). The
/// script builds it straight away, without asking for a folder, when the
/// note is under 15 minutes old, and deletes it.
pub fn build_note_file(config_dir: &Path) -> PathBuf {
    config_dir.join("resolve-build.txt")
}

pub fn write_build_note(config_dir: &Path, folder: &Path) -> Result<()> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    std::fs::create_dir_all(config_dir).with_context(|| format!("creating {}", config_dir.display()))?;
    std::fs::write(last_take_file(config_dir), folder.to_string_lossy().as_bytes()).context("noting the take")?;
    let path = build_note_file(config_dir);
    std::fs::write(&path, format!("{}\n{now}\n", folder.display())).with_context(|| format!("writing {}", path.display()))
}

/// Resolve seems to be on this computer: in a usual place, or (on a Mac,
/// installed elsewhere) its support folder is there.
pub fn resolve_present() -> bool {
    find_resolve().is_some() || (cfg!(target_os = "macos") && crate::config::home_dir().join("Library/Application Support/Blackmagic Design/DaVinci Resolve").is_dir())
}

/// The take's "(Resolve).lua", if it has one.
pub fn resolve_take_file(folder: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("(Resolve).lua"))).collect();
    found.sort();
    found.into_iter().next()
}

/// Makes sure the take has a Resolve take file with its cut list, for
/// Open in Resolve: one saved with the take is used as it is; an older
/// one without a cut list (or none at all) is written now from the take's
/// files and its Premiere timeline. Returns the file.
pub fn prepare_resolve_take(folder: &Path) -> Result<PathBuf> {
    let existing = resolve_take_file(folder);
    if let Some(p) = &existing {
        if std::fs::read_to_string(p).is_ok_and(|t| t.contains("\n  edit = {")) {
            return Ok(p.clone());
        }
    }
    let files: Vec<String> = std::fs::read_dir(folder).with_context(|| format!("reading {}", folder.display()))?.flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    let premiere = files.iter().find(|n| n.ends_with("(Premiere).xml")).and_then(|n| std::fs::read_to_string(folder.join(n)).ok()).and_then(|x| premiere_take(&x));
    if let (Some(p), None) = (&existing, &premiere) {
        return Ok(p.clone()); // nothing better to write
    }
    let premiere = premiere.unwrap_or_default();
    // The picture size and frame rate: from the timeline, or Take.txt ("Program: 1920x1080 @ 30 fps").
    let from_info = std::fs::read_to_string(folder.join("Take.txt")).ok().and_then(|t| {
        let line = t.lines().find(|l| l.starts_with("Program: "))?;
        let (size, rest) = line.trim_start_matches("Program: ").split_once(" @ ")?;
        let (w, h) = size.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?, rest.split_whitespace().next()?.parse().ok()?))
    });
    let (width, height, fps) = if premiere.fps > 0 { (premiere.width, premiere.height, premiere.fps) } else { from_info.unwrap_or((1920, 1080, 30)) };
    let is_video = |n: &String| {
        let l = n.to_lowercase();
        (l.ends_with(".mp4") || l.ends_with(".mov")) && !l.starts_with('.')
    };
    // Program first (then its clean copy), then the cameras in order.
    let mut cameras: Vec<String> = files.iter().filter(|n| is_video(n)).cloned().collect();
    cameras.sort_by_key(|n| (!n.starts_with("Program."), !n.starts_with("Program"), natural_key(n)));
    let marks: Vec<(f64, String)> = premiere.markers.iter().map(|(f, n)| (*f as f64 / fps.max(1) as f64, n.clone())).collect();
    let session = Session::from_folder(folder, &marks, None, fps)?;
    let mut edit = premiere.edit;
    if !session.tracks.iter().any(|t| t.file == "Mix.wav") {
        edit.sound = cameras.iter().find(|n| n.starts_with("Program.")).cloned();
    }
    let timeline = take_timeline(folder).and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
    let text = session.resolve_lua(folder, &cameras, timeline.as_deref(), (width, height), &edit);
    let path = existing.unwrap_or_else(|| folder.join(format!("{} (Resolve).lua", session.name)));
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(path: &Path, channels: u16, frames: usize) {
        let mut w = crate::media::wav::WavWriter::create(path, crate::media::wav::SampleFormat::from_bits(24), channels, 48_000).unwrap();
        w.write(&vec![0.1f32; frames * channels as usize]).unwrap();
        w.finish().unwrap();
    }

    fn take(tag: &str) -> (PathBuf, Session) {
        let dir = std::env::temp_dir().join(format!("cutaway-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Take_2026-10-18_20-00-00")).unwrap();
        let folder = dir.join("Take_2026-10-18_20-00-00");
        wav(&folder.join("Audio_01_DAW.wav"), 2, 96_000);
        wav(&folder.join("Audio_02_Guest_ch2.wav"), 1, 96_000);
        wav(&folder.join("Audio_02_Guest_ch10.wav"), 1, 96_000);
        wav(&folder.join("Mix.wav"), 2, 96_000);
        std::fs::write(folder.join("Program.mp4"), b"x").unwrap();
        let s = Session::from_folder(&folder, &[(1.5, "Intro & \"hello\"".into())], Some(72_000.0), 30).unwrap();
        (folder, s)
    }

    #[test]
    fn reads_a_take() {
        let (folder, s) = take("read");
        let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["DAW", "Guest 2", "Guest 10", "Mix"]);
        assert!(s.tracks[3].muted && !s.tracks[0].muted);
        assert_eq!((s.tracks[0].channels, s.tracks[1].channels), (2, 1));
        assert!((s.tracks[0].seconds - 2.0).abs() < 1e-9);
        assert_eq!(s.video.as_ref().map(|v| v.0.as_str()), Some("Program.mp4"));
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    #[test]
    fn writes_reaper_and_dawproject() {
        let (folder, s) = take("write");
        let rpp = s.reaper();
        assert!(rpp.starts_with("<REAPER_PROJECT") && rpp.trim_end().ends_with('>'));
        assert_eq!(rpp.matches("<TRACK").count(), 5, "video + 4 audio tracks");
        assert!(rpp.contains("FILE \"Audio_02_Guest_ch10.wav\""));
        assert!(rpp.contains("MARKER 1 1.500000 \"Intro & 'hello'\""));
        assert!(rpp.contains("PROJOFFS 72000.000000"));
        // Balanced blocks.
        assert_eq!(rpp.matches('<').count(), rpp.lines().filter(|l| l.trim() == ">").count());

        let xml = s.dawproject_xml();
        assert!(xml.contains("<File path=\"Mix.wav\" external=\"true\"/>"));
        assert!(xml.contains("<Marker time=\"1.500000\" name=\"Intro &amp; &quot;hello&quot;\"/>"));
        assert_eq!(xml.matches("<Clip ").count(), 4);
        let path = folder.join("Take.dawproject");
        s.write_dawproject(&path).unwrap();
        let zip = std::fs::read(&path).unwrap();
        assert_eq!(&zip[..4], b"PK\x03\x04");
        assert!(zip.windows(11).any(|w| w == b"project.xml"));

        let lua = s.resolve_lua(&folder, &["Program.mp4".into()], Some("Take.fcpxml"), (1920, 1080), &ResolveEdit::default());
        assert!(!lua.contains("edit = {"), "no cut list: the script imports the .fcpxml");
        assert!(lua.contains("fps = 30,") && lua.contains("width = 1920,") && lua.contains("height = 1080,") && lua.contains("Program.mp4") && lua.contains("frame = 45"));
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    #[test]
    fn names_tracks() {
        assert_eq!(track_name("Audio_01_DAW.wav"), "DAW");
        assert_eq!(track_name("Audio_03_Host_Mic_ch2.wav"), "Host Mic 2");
        assert_eq!(track_name("Mix.wav"), "Mix");
    }

    /// Open in Resolve hands Resolve the take's .fcpxml, and knows where
    /// Resolve lives on this platform.
    #[test]
    fn finds_the_timeline_for_resolve() {
        let (folder, _) = take("resolve");
        assert_eq!(take_timeline(&folder), None, "no timeline saved");
        std::fs::write(folder.join("Take_2026-10-18_20-00-00 (Premiere).xml"), "x").unwrap();
        std::fs::write(folder.join("Take_2026-10-18_20-00-00.fcpxml"), "x").unwrap();
        assert_eq!(take_timeline(&folder), Some(folder.join("Take_2026-10-18_20-00-00.fcpxml")));
        let places = resolve_app_candidates();
        assert!(!places.is_empty());
        if cfg!(target_os = "macos") {
            assert!(places[0].ends_with("DaVinci Resolve/DaVinci Resolve.app"));
        } else if cfg!(windows) {
            assert!(places.iter().all(|p| p.ends_with("Resolve.exe")));
        }
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }
    /// A take of three files: Program, Wide (30 fps) and Host (25 fps),
    /// 10 s, cut Wide → Side by side (the program) → Host.
    fn export_of(folder: &Path) -> crate::engine::export::TakeExport {
        use crate::engine::export::{Cut, ExportFile, TakeExport};
        let file = |name: &str, source: Option<usize>, fps| ExportFile {
            path: folder.join(format!("{name}.mp4")),
            name: name.replace("ISO_01_", "").replace("ISO_02_", ""),
            width: 1920,
            height: 1080,
            fps,
            has_audio: true,
            source,
            tc_frame: Some(37230 * fps),
            length: None,
            clean: false,
        };
        TakeExport {
            name: folder.file_name().unwrap().to_string_lossy().into(),
            fps: 30,
            width: 1920,
            height: 1080,
            frames: 300,
            files: vec![file("Program", None, 30), file("ISO_01_Wide", Some(0), 30), file("ISO_02_Host", Some(1), 25)],
            cuts: vec![
                Cut { frame: 0, angle: Some(0), scene: "Wide".into() },
                Cut { frame: 90, angle: None, scene: "Side by side".into() },
                Cut { frame: 180, angle: Some(1), scene: "Host & \"Co\"".into() },
            ],
            markers: vec![(120, "Intro done".into())],
        }
    }

    /// The Premiere timeline and the Resolve take file carry the same
    /// shots; an older take's Premiere timeline gives them back.
    #[test]
    fn the_cut_list_reads_back_from_the_premiere_timeline() {
        let te = export_of(Path::new("/tmp/Take 1"));
        let edit = ResolveEdit::from_export(&te, true);
        assert_eq!(edit.cuts.iter().map(|c| (c.file.as_str(), c.at, c.length, c.from, c.to)).collect::<Vec<_>>(), vec![("ISO_01_Wide.mp4", 0, 90, 0, 90), ("Program.mp4", 90, 90, 90, 180), ("ISO_02_Host.mp4", 180, 120, 150, 250)]);
        assert_eq!(edit.angles.iter().map(|a| (a.name.as_str(), a.frames)).collect::<Vec<_>>(), vec![("Program", 300), ("Wide", 300), ("Host", 250)]);
        let back = premiere_take(&crate::engine::export::premiere_xml(&te)).unwrap();
        assert_eq!((back.fps, back.width, back.height), (30, 1920, 1080));
        assert_eq!(back.edit.cuts, edit.cuts);
        assert_eq!(back.edit.angles, edit.angles);
        assert_eq!(back.edit.frames, 300);
        assert_eq!(back.markers, vec![(120, "Intro done".to_string())]);
    }

    /// A Lua to run the script with: Resolve's own is LuaJIT. None when
    /// there's none here (the test then checks nothing more).
    fn lua() -> Option<&'static str> {
        ["luajit", "lua5.1", "lua"].into_iter().find(|l| std::process::Command::new(l).arg("-v").output().is_ok_and(|o| o.status.success()))
    }

    /// Runs the script in the stand-in for Resolve; its calls, one a line.
    fn run_script(config_dir: &Path, picker: &str, mode: &str) -> Option<Vec<String>> {
        let lua = lua()?;
        let dir = config_dir.join("script");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("script.lua"), RESOLVE_SCRIPT).unwrap();
        std::fs::write(dir.join("harness.lua"), include_str!("resolve_script_harness.lua")).unwrap();
        let o = std::process::Command::new(lua).arg(dir.join("harness.lua")).arg(dir.join("script.lua")).arg(config_dir).arg(picker).arg(mode).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        Some(String::from_utf8_lossy(&o.stdout).lines().map(String::from).collect())
    }

    fn appends(calls: &[String]) -> Vec<&str> {
        calls.iter().filter(|c| c.starts_with("APPEND")).map(|c| c.as_str()).collect()
    }

    /// Open in Resolve's note makes the script build that take without
    /// asking: the cuts on V1, every angle on its own track (off), the mix
    /// under it, the markers; the note is used up.
    #[test]
    fn the_resolve_script_builds_the_live_edit() {
        let (folder, s) = take("script");
        let config = folder.parent().unwrap().join("config");
        let te = export_of(&folder);
        for f in &te.files {
            std::fs::write(&f.path, b"x").unwrap();
        }
        let videos: Vec<String> = te.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into()).collect();
        let edit = ResolveEdit::from_export(&te, true);
        let text = s.resolve_lua(&folder, &videos, Some("Take.fcpxml"), (1920, 1080), &edit);
        std::fs::write(folder.join(format!("{} (Resolve).lua", s.name)), &text).unwrap();
        write_build_note(&config, &folder).unwrap();
        let Some(calls) = run_script(&config, "-", "inclusive") else { return };
        let all = calls.join("\n");
        assert!(!all.contains("PICKER"), "the note answers which take: {all}");
        assert!(!build_note_file(&config).exists(), "the note is used up");
        assert!(all.contains("SETTING\ttimelineFrameRate\t30") && all.contains("SETTING\ttimelineResolutionWidth\t1920"));
        let settings_at = all.find("SETTING\ttimelineFrameRate").unwrap();
        assert!(settings_at < all.find("IMPORT\t").unwrap() && settings_at < all.find("TIMELINE\t").unwrap(), "frame rate before media and timeline");
        assert_eq!(
            appends(&calls),
            vec![
                // V1, the live edit (endFrame is the last frame used).
                "APPEND\tISO_01_Wide.mp4\tfrom=0\tend=89\trecord=108000\ttrack=1\tmedia=1",
                "APPEND\tProgram.mp4\tfrom=90\tend=179\trecord=108090\ttrack=1\tmedia=1",
                "APPEND\tISO_02_Host.mp4\tfrom=150\tend=249\trecord=108180\ttrack=1\tmedia=1",
                // Every angle full length from the start, a track each.
                "APPEND\tProgram.mp4\tfrom=0\tend=299\trecord=108000\ttrack=2\tmedia=1",
                "APPEND\tISO_01_Wide.mp4\tfrom=0\tend=299\trecord=108000\ttrack=3\tmedia=1",
                "APPEND\tISO_02_Host.mp4\tfrom=0\tend=249\trecord=108000\ttrack=4\tmedia=1",
                // The mix under it.
                "APPEND\tMix.wav\tfrom=0\tend=299\trecord=108000\ttrack=1\tmedia=2",
            ]
        );
        for (i, n) in [(2, "Program"), (3, "Wide"), (4, "Host")] {
            assert!(all.contains(&format!("TRACKNAME\tvideo\t{i}\t{n}")) && all.contains(&format!("TRACKENABLE\tvideo\t{i}\tfalse")), "{all}");
        }
        assert!(all.contains("MARKER\t45\tIntro & \"hello\""), "{all}");
        assert!(!all.contains("IMPORTTIMELINE"), "{all}");
        assert!(all.contains("SAVE") && all.contains("every camera on its own track"), "{all}");

        // A Resolve that counts endFrame the other way: put right after the first shot.
        write_build_note(&config, &folder).unwrap();
        let calls = run_script(&config, "-", "exclusive").unwrap();
        let a = appends(&calls);
        assert_eq!(a[0], "APPEND\tISO_01_Wide.mp4\tfrom=0\tend=89\trecord=108000\ttrack=1\tmedia=1");
        assert!(calls.iter().any(|c| c == "DELETECLIPS\t1"));
        assert_eq!(a[1], "APPEND\tISO_01_Wide.mp4\tfrom=0\tend=90\trecord=108000\ttrack=1\tmedia=1");
        assert_eq!(a[2], "APPEND\tProgram.mp4\tfrom=90\tend=180\trecord=108090\ttrack=1\tmedia=1");

        // Clip by clip fails: the .fcpxml instead (and no half timeline).
        write_build_note(&config, &folder).unwrap();
        let all = run_script(&config, "-", "broken").unwrap().join("\n");
        assert!(all.contains("DELETETIMELINES\t1") && all.contains("IMPORTTIMELINE\tTake.fcpxml"), "{all}");

        // An older take file (no cut list), and no note: the picker, at the latest take.
        std::fs::write(folder.join(format!("{} (Resolve).lua", s.name)), s.resolve_lua(&folder, &videos, Some("Take.fcpxml"), (1920, 1080), &ResolveEdit::default())).unwrap();
        std::fs::write(last_take_file(&config), folder.to_string_lossy().as_bytes()).unwrap();
        let all = run_script(&config, &folder.to_string_lossy(), "inclusive").unwrap().join("\n");
        assert!(all.contains(&format!("PICKER\t{}", folder.display())), "{all}");
        assert!(!all.contains("APPEND") && all.contains("IMPORTTIMELINE\tTake.fcpxml"), "{all}");

        // A stale note is ignored (and cleared).
        std::fs::write(build_note_file(&config), format!("{}\n{}\n", folder.display(), 1_000_000)).unwrap();
        let all = run_script(&config, "-", "inclusive").unwrap().join("\n");
        assert!(all.contains("PICKER") && all.contains("no folder chosen"), "{all}");
        assert!(!build_note_file(&config).exists());
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    /// Open in Resolve on an older take: its take file gets the cut list
    /// from the take's Premiere timeline.
    #[test]
    fn an_older_take_gets_its_cut_list() {
        let (folder, s) = take("older");
        let te = export_of(&folder);
        std::fs::write(folder.join(format!("{} (Premiere).xml", s.name)), crate::engine::export::premiere_xml(&te)).unwrap();
        std::fs::write(folder.join(format!("{}.fcpxml", s.name)), "x").unwrap();
        std::fs::write(folder.join("ISO_01_Wide.mp4"), "x").unwrap();
        let old = s.resolve_lua(&folder, &["Program.mp4".into()], None, (1280, 720), &ResolveEdit::default());
        let file = folder.join(format!("{} (Resolve).lua", s.name));
        std::fs::write(&file, old).unwrap();
        assert_eq!(prepare_resolve_take(&folder).unwrap(), file);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\n  edit = {") && text.contains("scene = \"Host & \\\"Co\\\"\", at = 180, length = 120, from = 150, to = 250"), "{text}");
        assert!(text.contains("frames = 300,") && text.contains("Mix.wav") && text.contains("width = 1920,"), "{text}");
        assert!(text.contains("cameras = { \"") && text.find("Program.mp4").unwrap() < text.find("ISO_01_Wide.mp4").unwrap(), "{text}");
        assert!(text.contains("{ frame = 120, name = \"Intro done\" }"), "{text}");
        // Already has one: left as it is.
        std::fs::write(&file, "x\n  edit = {").unwrap();
        prepare_resolve_take(&folder).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x\n  edit = {");
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    /// A real take: CUTAWAY_RESOLVE_TAKE=<folder> cargo test -- --ignored resolve_on_a_real_take --nocapture
    #[test]
    #[ignore]
    fn resolve_on_a_real_take() {
        let Some(folder) = std::env::var_os("CUTAWAY_RESOLVE_TAKE").map(PathBuf::from) else { return };
        let file = prepare_resolve_take(&folder).unwrap();
        println!("{}", std::fs::read_to_string(&file).unwrap());
        let config = std::env::temp_dir().join(format!("cutaway-real-take-{}", std::process::id()));
        write_build_note(&config, &folder).unwrap();
        for l in run_script(&config, "-", "inclusive").unwrap_or_default() {
            println!("{l}");
        }
        let _ = std::fs::remove_dir_all(&config);
    }
}
