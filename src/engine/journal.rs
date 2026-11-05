//! The take's journal: a few lines in the take folder, written as the
//! take goes (`.take-journal.jsonl`), so a take rescued after a crash or a
//! power cut gets back what otherwise lives only in memory until Stop:
//! the switches (the live edit), the markers and the files' details.
//!
//! One JSON object a line: the take's start first, then each switch,
//! marker (and marker rename or delete) and file change as it happens.
//! Each line goes to disk straight away, on a thread of its own so a
//! switch never waits for the drive. At a normal Stop the side files are
//! written as usual and the journal is removed; the rescue reads it back
//! and writes the same side files (`take::rebuild_from_journal`).

use super::export::ExportFile;
use super::recorder::Marker;
use super::switcher::SwitchEvent;
use crate::config::OutputConfig;
use crate::media::Ns;
use crate::util::LockExt;
use crossbeam_channel::{unbounded, Sender};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;

/// The journal's file name in the take folder.
pub const FILE: &str = ".take-journal.jsonl";

/// What the take started with: everything the side files need besides the
/// switches and markers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    /// The take's name (its folder's).
    pub take: String,
    /// Host time of the files' first frame (switch times are host times).
    pub t0: Ns,
    pub fps: u32,
    pub canvas: (u32, u32),
    /// Wall clock at the first frame (RFC 3339).
    pub wall: String,
    /// Time of day at the first frame, in seconds since midnight.
    pub tc_seconds: f64,
    pub first_scene: String,
    pub first_angle: Option<usize>,
    /// Every video file (paths are file names in the take folder).
    pub files: Vec<ExportFile>,
    /// The setup summary for Take.txt.
    pub setup: String,
    /// The recording settings the take was made with.
    pub output: OutputConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Start(Box<Start>),
    Switch(SwitchEvent),
    Marker(Marker),
    /// The marker at `at` renamed.
    Rename { at: Ns, name: String },
    /// The marker at `at` taken out.
    Delete { at: Ns },
    /// A source's file ended `length` into the take (it left the show).
    Ends { source: usize, length: Ns },
    /// A file started during the take (path is a file name).
    File(ExportFile),
    /// A close-up saved as a crop moved: its shot from `at` (from the
    /// start of the take), as fractions of the wide picture.
    Shot { source: usize, at: Ns, rect: [f32; 4] },
}

/// Writes the journal to the take folder (and its backup copy).
pub struct Journal {
    tx: Mutex<Option<Sender<String>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    paths: Vec<PathBuf>,
}

fn line(e: &Entry) -> Option<String> {
    serde_json::to_string(e).ok().map(|mut s| {
        s.push('\n');
        s
    })
}

impl Journal {
    /// Starts the journal in each of `dirs` with the take's start. A
    /// folder where it can't be written is skipped (the take goes on; only
    /// a rescue would miss it). None when it couldn't be written anywhere.
    pub fn create(dirs: &[&Path], start: Start) -> Option<Journal> {
        let first = line(&Entry::Start(Box::new(start)))?;
        let mut files = Vec::new();
        let mut paths = Vec::new();
        for d in dirs {
            let p = d.join(FILE);
            let Ok(mut f) = File::create(&p) else { continue };
            if f.write_all(first.as_bytes()).is_ok() {
                let _ = f.sync_data();
                files.push(f);
                paths.push(p);
            }
        }
        if files.is_empty() {
            return None;
        }
        let (tx, rx) = unbounded::<String>();
        let thread = std::thread::Builder::new()
            .name("take journal".into())
            .spawn(move || {
                while let Ok(l) = rx.recv() {
                    let mut batch = l;
                    while let Ok(more) = rx.try_recv() {
                        batch.push_str(&more);
                    }
                    for f in &mut files {
                        if f.write_all(batch.as_bytes()).is_ok() {
                            let _ = f.sync_data();
                        }
                    }
                }
            })
            .ok()?;
        Some(Journal { tx: Mutex::new(Some(tx)), thread: Mutex::new(Some(thread)), paths })
    }

    /// Adds a line (never waits for the drive).
    pub fn add(&self, e: &Entry) {
        if let (Some(tx), Some(l)) = (self.tx.lock_ok().as_ref(), line(e)) {
            let _ = tx.send(l);
        }
    }

    /// Writes what's waiting and stops.
    pub fn close(&self) {
        self.tx.lock_ok().take();
        if let Some(t) = self.thread.lock_ok().take() {
            let _ = t.join();
        }
    }

    /// The take ended normally and its side files are written: the journal
    /// isn't needed any more.
    pub fn remove(&self) {
        self.close();
        for p in &self.paths {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.close();
    }
}

/// A journal read back.
#[derive(Clone, Debug)]
pub struct Replay {
    pub start: Start,
    /// Switches in the order made.
    pub events: Vec<SwitchEvent>,
    /// Markers as they stood at the end, in time order.
    pub markers: Vec<Marker>,
    /// Every video file, with where each ended, and files started later.
    pub files: Vec<ExportFile>,
}

/// Reads a take's journal. A last line cut short by the crash is left out.
/// None when there's no journal (or its start is unreadable).
pub fn read(dir: &Path) -> Option<Replay> {
    let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
    parse(&text)
}

pub fn parse(text: &str) -> Option<Replay> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Entry::Start(start) = serde_json::from_str::<Entry>(lines.next()?).ok()? else { return None };
    let mut r = Replay { files: start.files.clone(), start: *start, events: Vec::new(), markers: Vec::new() };
    for l in lines {
        let Ok(e) = serde_json::from_str::<Entry>(l) else { continue };
        match e {
            Entry::Start(_) => {}
            Entry::Switch(s) => r.events.push(s),
            Entry::Marker(m) => {
                r.markers.push(m);
                r.markers.sort_by_key(|m| m.at);
            }
            Entry::Rename { at, name } => {
                if let Some(m) = r.markers.iter_mut().find(|m| m.at == at) {
                    m.name = name;
                }
            }
            Entry::Delete { at } => r.markers.retain(|m| m.at != at),
            Entry::Ends { source, length } => {
                // The latest file of that source (an earlier one may have
                // ended already).
                if let Some(f) = r.files.iter_mut().rev().find(|f| f.source == Some(source) && f.length.is_none()) {
                    f.length = Some(length);
                }
            }
            Entry::File(f) => r.files.push(f),
            Entry::Shot { source, at, rect } => {
                for c in r.files.iter_mut().filter(|f| f.source == Some(source)).filter_map(|f| f.crop.as_mut()) {
                    c.keys.push((at, rect));
                }
            }
        }
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransitionMode;

    fn start() -> Start {
        Start {
            take: "Show_2026-11-01_20-00-00".into(),
            t0: 5_000_000_000,
            fps: 30,
            canvas: (1920, 1080),
            wall: "2026-11-01T20:00:00+00:00".into(),
            tc_seconds: 72_000.0,
            first_scene: "Wide".into(),
            first_angle: Some(1),
            files: Vec::new(),
            setup: String::new(),
            output: OutputConfig::default(),
        }
    }

    #[test]
    fn written_and_read_back() {
        let dir = std::env::temp_dir().join(format!("cutaway-journal-{}", std::process::id()));
        let backup = dir.join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        let j = Journal::create(&[&dir, &backup], start()).unwrap();
        j.add(&Entry::Switch(SwitchEvent { at: 6_000_000_000, scene: "Host".into(), mode: TransitionMode::Cut, duration_ms: 0, angle: Some(2) }));
        let m = |at, name: &str| Marker { at, name: name.into(), scene: "Host".into(), clip: false };
        j.add(&Entry::Marker(m(3_000_000_000, "Marker 2")));
        j.add(&Entry::Marker(m(1_000_000_000, "Marker 1")));
        j.add(&Entry::Rename { at: 3_000_000_000, name: "News".into() });
        j.add(&Entry::Delete { at: 1_000_000_000 });
        j.close();
        // A crash halfway through a line.
        let mut text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        text.push_str("{\"kind\":\"marker\",\"at\":9");
        std::fs::write(dir.join(FILE), &text).unwrap();
        for d in [&dir, &backup] {
            let r = read(d).unwrap();
            assert_eq!(r.start.first_scene, "Wide");
            assert_eq!(r.events.len(), 1);
            assert_eq!(r.markers.iter().map(|m| (m.at, m.name.as_str())).collect::<Vec<_>>(), vec![(3_000_000_000, "News")]);
        }
        j.remove();
        assert!(!dir.join(FILE).exists() && !backup.join(FILE).exists());
        assert!(read(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A close-up saved as a crop: its shots come back with its entry
    /// (and one that joined later gets those from then on too).
    #[test]
    fn crop_shots_read_back() {
        use crate::engine::export::Crop;
        let crop = |start| ExportFile {
            path: "ISO_01_Cam_1.mp4".into(),
            name: "Cam 1 left".into(),
            width: 3840,
            height: 2160,
            fps: 30,
            has_audio: false,
            source: Some(7),
            tc_frame: None,
            length: None,
            clean: false,
            start,
            crop: Some(Crop { of: 3, size: (1920, 1080), keys: vec![(start, [0.0, 0.25, 0.5, 0.5])] }),
        };
        let mut s = start();
        s.files = vec![crop(0)];
        let mut text = line(&Entry::Start(Box::new(s))).unwrap();
        text += &line(&Entry::Shot { source: 7, at: 2_000_000_000, rect: [0.5, 0.25, 0.5, 0.5] }).unwrap();
        text += &line(&Entry::Shot { source: 9, at: 2_000_000_000, rect: [0.0, 0.0, 1.0, 1.0] }).unwrap();
        let r = parse(&text).unwrap();
        assert_eq!(r.files[0].crop.as_ref().unwrap().keys, vec![(0, [0.0, 0.25, 0.5, 0.5]), (2_000_000_000, [0.5, 0.25, 0.5, 0.5])]);
        // An older Cutaway's journal (no crops) reads as before.
        let mut plain = start();
        plain.files = vec![ExportFile { crop: None, ..crop(0) }];
        let j = line(&Entry::Start(Box::new(plain))).unwrap();
        assert!(!j.contains("crop"));
        assert!(parse(&j).unwrap().files[0].crop.is_none());
    }
}
