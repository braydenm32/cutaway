//! Hands a take to audio and video software as a ready-made project:
//!
//! * `<take>.rpp` for REAPER: every WAV on its own track, lined up, with
//!   the program video on top and the markers.
//! * `<take>.dawproject` for Fender Studio Pro (Studio One), Bitwig and
//!   Cubase: the open DAWproject format, referring to the WAVs next to it.
//! * `<take> (Resolve).lua`: what Cutaway's DaVinci Resolve script reads to
//!   build a project (bins, media, proxies, the live edit). The script
//!   itself is installed once from Settings → Recording.
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

    fn length(&self) -> f64 {
        self.tracks.iter().map(|t| t.seconds).chain(self.video.iter().map(|v| v.1)).fold(0.0, f64::max)
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

    /// What the Resolve script reads: the take as a Lua table.
    pub fn resolve_lua(&self, folder: &Path, cameras: &[String], timeline: Option<&str>) -> String {
        let lua = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " "));
        let path = |f: &str| lua(&folder.join(f).to_string_lossy());
        let mut s = String::from("-- Written by Cutaway: read by the \"Cutaway - Build project from a take\" script in DaVinci Resolve.\nreturn {\n");
        let _ = writeln!(s, "  name = {},", lua(&self.name));
        let _ = writeln!(s, "  fps = {},", self.fps);
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
        s.push_str(" },\n}\n");
        s
    }
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

/// Where Cutaway notes the latest take, for the Resolve script's default.
pub fn last_take_file(config_dir: &Path) -> PathBuf {
    config_dir.join("last-take.txt")
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

        let lua = s.resolve_lua(&folder, &["Program.mp4".into()], Some("Take.fcpxml"));
        assert!(lua.contains("fps = 30,") && lua.contains("Program.mp4") && lua.contains("frame = 45"));
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    #[test]
    fn names_tracks() {
        assert_eq!(track_name("Audio_01_DAW.wav"), "DAW");
        assert_eq!(track_name("Audio_03_Host_Mic_ch2.wav"), "Host Mic 2");
        assert_eq!(track_name("Mix.wav"), "Mix");
    }
}
