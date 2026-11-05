//! Repairs takes that were never stopped properly (crash, power cut, force
//! quit). Video files are written as fragments while recording, so
//! everything up to the last second is on disk; this turns them back into
//! ordinary MP4/MOV files and fixes the WAV headers.

use crate::media::{mp4, wav};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use crate::util::LockExt;

fn is_media(p: &Path, exts: &[&str]) -> bool {
    p.extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Where takes are recorded: the output folder and the backup folder.
pub fn folders(cfg: &crate::config::Config) -> Vec<PathBuf> {
    folders_of(&cfg.output)
}

/// `folders`, from just the output settings (so a worker can take a copy).
pub fn folders_of(output: &crate::config::OutputConfig) -> Vec<PathBuf> {
    let mut folders = vec![output.folder.clone()];
    folders.extend(output.backup_folder.clone().filter(|p| !p.as_os_str().is_empty()));
    // The backup set to the same folder: look there once.
    let same = |a: &Path, b: &Path| a == b || fs::canonicalize(a).ok().is_some_and(|a| fs::canonicalize(b).ok() == Some(a));
    if folders.len() == 2 && same(&folders[0], &folders[1]) {
        folders.pop();
    }
    folders
}

/// Where Cutaway notes the folders takes were recorded to (newest first),
/// so Record & stream › Recordings lists last week's takes after the folder changes.
fn folders_file(config_dir: &Path) -> PathBuf {
    config_dir.join("take-folders.txt")
}

/// How many recording folders are remembered.
const REMEMBERED: usize = 8;

/// Note that takes go to `folders` (the recording folder, then its backup).
pub fn remember_folders(config_dir: &Path, folders: &[PathBuf]) {
    let mut list: Vec<PathBuf> = folders.to_vec();
    for f in remembered_folders(config_dir) {
        if !list.contains(&f) {
            list.push(f);
        }
    }
    list.truncate(REMEMBERED);
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = fs::create_dir_all(config_dir);
    let _ = fs::write(folders_file(config_dir), text);
}

/// The folders takes were recorded to, newest first.
pub fn remembered_folders(config_dir: &Path) -> Vec<PathBuf> {
    fs::read_to_string(folders_file(config_dir)).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

/// A take that was never stopped properly, and its media files.
pub struct Unfinished {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

/// A take's media files: its own folder and its Proxies folder.
fn media_in(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for d in [dir.to_path_buf(), dir.join("Proxies")] {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        files.extend(rd.flatten().map(|f| f.path()).filter(|p| p.is_file() && is_media(p, &["mp4", "mov", "wav"]) && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))));
    }
    files.sort();
    files
}

/// Takes in `output` that need repairing (just a look at the folders: quick).
pub fn find_unfinished(output: &Path) -> Vec<Unfinished> {
    let Ok(entries) = fs::read_dir(output) else { return Vec::new() };
    let mut takes = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || dir.join("Take.txt").exists() {
            continue;
        }
        let files = media_in(&dir);
        let ours = files.iter().any(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("Program.") || name.starts_with("ISO_") || name.starts_with("Audio_")
        }) || dir.join(crate::engine::journal::FILE).exists();
        if ours && !files.is_empty() {
            takes.push(Unfinished { dir, files });
        }
    }
    takes.sort_by(|a, b| a.dir.cmp(&b.dir));
    takes
}

/// "Show_2026-11-01_20-04-12" (or "…_2") → when it was recorded.
fn stamp_of(name: &str) -> Option<chrono::NaiveDateTime> {
    let parts: Vec<&str> = name.split('_').collect();
    (0..parts.len().saturating_sub(1)).rev().find_map(|i| chrono::NaiveDateTime::parse_from_str(&format!("{} {}", parts[i], parts[i + 1]), "%Y-%m-%d %H-%M-%S").ok())
}

/// Repairs one take's files, calling `on_file` with each file's index and
/// name before it's worked on, and marks the take finished (Take.txt).
/// With its journal, the take gets back its side files too (switch log,
/// timelines, Resolve take file, chapters, markers).
pub fn repair(take: &Unfinished, mut on_file: impl FnMut(usize, &str)) -> bool {
    let journal = crate::engine::journal::read(&take.dir);
    // Chapters in the files, as Stop puts them there.
    let chapters: Vec<(i64, String)> = journal.as_ref().map(|j| j.markers.iter().filter(|m| !m.clip).map(|m| (m.at, m.name.clone())).collect()).unwrap_or_default();
    let mut lines = Vec::new();
    // The files that play, their sizes, and how long each is.
    let mut good: Vec<(PathBuf, u64)> = Vec::new();
    let mut seconds: Vec<(PathBuf, f64)> = Vec::new();
    for (i, p) in take.files.iter().enumerate() {
        let name = p.strip_prefix(&take.dir).unwrap_or(p).to_string_lossy().to_string();
        on_file(i, &name);
        // A damaged file mustn't stop the rest (or the rescue screen).
        let length = if is_media(p, &["mp4", "mov"]) {
            match crate::util::guard("repairing a recording", || mp4::recover_with(p, &chapters)) {
                Ok(mp4::Recovery::Recovered { seconds }) => {
                    lines.push(format!("  {name}: recovered {seconds:.1} s"));
                    Some(seconds)
                }
                Ok(mp4::Recovery::AlreadyComplete) => {
                    lines.push(format!("  {name}: complete"));
                    mp4::duration_secs(p)
                }
                Ok(mp4::Recovery::NotRecoverable) => {
                    lines.push(format!("  {name}: nothing could be recovered"));
                    None
                }
                Err(e) => {
                    lines.push(format!("  {name}: couldn't repair ({e:#})"));
                    None
                }
            }
        } else {
            let ok = match crate::util::guard("repairing a recording", || wav::repair(p)) {
                Ok(true) => {
                    lines.push(format!("  {name}: header repaired"));
                    true
                }
                Ok(false) => {
                    lines.push(format!("  {name}: complete"));
                    true
                }
                Err(e) => {
                    lines.push(format!("  {name}: couldn't repair ({e:#})"));
                    false
                }
            };
            ok.then(|| wav::WavReader::open(p).ok().map(|r| r.frames as f64 / r.rate.max(1) as f64)).flatten()
        };
        if let Some(secs) = length {
            good.push((p.clone(), fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
            seconds.push((p.clone(), secs));
        }
    }
    // The take's length: the program's, else the longest file's.
    let is_program = |p: &Path| p.parent() == Some(take.dir.as_path()) && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Program."));
    let secs = seconds.iter().find(|(p, _)| is_program(p)).map(|s| s.1).unwrap_or_else(|| seconds.iter().map(|s| s.1).fold(0.0, f64::max));
    let length = std::time::Duration::from_secs_f64(secs.max(0.0));
    if let Some(j) = &journal {
        crate::engine::take::rebuild_from_journal(&take.dir, j, good, length, lines);
        let _ = fs::remove_file(take.dir.join(crate::engine::journal::FILE));
        return take.dir.join("Take.txt").exists();
    }
    // No journal (a take from an older Cutaway): what the files say.
    let name = take.dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut note = String::from("Cutaway take (recovered)\n");
    if let Some(at) = stamp_of(&name) {
        note += &format!("Recorded: {}\n", at.format("%Y-%m-%d %H:%M"));
    }
    note += &format!("Length: {}\n", crate::engine::fmt_duration(length));
    note += "\nThis take wasn't stopped normally (crash, power loss or force quit).\n\
             Files play up to about the last second that was written. Its switches and markers weren't saved,\n\
             so it has no timelines or chapters.\n\nFiles:\n";
    for (p, size) in &good {
        let rel = p.strip_prefix(&take.dir).unwrap_or(p).to_string_lossy().to_string();
        note += &format!("  {rel}  ({})\n", crate::util::human_size(*size));
    }
    note += &format!("\nRepair:\n{}\n", lines.join("\n"));
    fs::write(take.dir.join("Take.txt"), note).is_ok()
}

/// Finds unfinished takes in `output` and repairs them. Returns their folders.
pub fn recover_unfinished(output: &Path) -> Vec<PathBuf> {
    find_unfinished(output).into_iter().filter(|t| repair(t, |_, _| {})).map(|t| t.dir).collect()
}

/// How a rescue in the background is getting on.
#[derive(Clone, Default)]
pub struct RescueState {
    /// Still looking for unfinished takes (done in the background, as a
    /// big or cold drive can be slow); nothing shows until it is false.
    pub scanning: bool,
    /// The takes being rescued, and which one it's on.
    pub takes: Vec<PathBuf>,
    pub take: usize,
    /// The file it's on: its name, its number in the take, and how many.
    pub file: String,
    pub file_index: usize,
    pub files: usize,
    /// Files finished, out of all of them.
    pub files_done: usize,
    pub files_total: usize,
    /// Takes that were repaired (and marked), once done.
    pub rescued: Vec<PathBuf>,
    pub finished: bool,
}

impl RescueState {
    /// 0 to 1, by file.
    pub fn fraction(&self) -> f32 {
        if self.finished || self.files_total == 0 {
            1.0
        } else {
            self.files_done as f32 / self.files_total as f32
        }
    }
}

/// Looks for unfinished takes in these folders and rescues them, all in the
/// background. The state says `scanning` first; when that ends with no takes
/// there was nothing to rescue.
pub fn start_rescue(folders: &[PathBuf], waker: crate::engine::Waker) -> Arc<Mutex<RescueState>> {
    let state = Arc::new(Mutex::new(RescueState { scanning: true, ..Default::default() }));
    let s2 = state.clone();
    let folders2 = folders.to_vec();
    let spawned = std::thread::Builder::new().name("rescue".into()).spawn(move || {
        let takes: Vec<Unfinished> = folders2.iter().flat_map(|f| find_unfinished(f)).collect();
        {
            let mut st = s2.lock_ok();
            st.takes = takes.iter().map(|t| t.dir.clone()).collect();
            st.files_total = takes.iter().map(|t| t.files.len()).sum();
            st.scanning = false;
            st.finished = takes.is_empty();
        }
        waker();
        if takes.is_empty() {
            return;
        }
        // (Screenshots of this screen: go slowly.)
        let slow = std::env::var("CUTAWAY_UI_TEST_SLOW_RESCUE").ok().and_then(|v| v.parse::<f64>().ok()).map(|s| std::time::Duration::from_secs_f64(s.clamp(0.0, 30.0)));
        let mut done = 0;
        for (k, take) in takes.iter().enumerate() {
            let ok = repair(take, |i, name| {
                if let Some(d) = slow {
                    std::thread::sleep(d);
                }
                let mut st = s2.lock_ok();
                st.take = k;
                st.file = name.to_string();
                st.file_index = i;
                st.files = take.files.len();
                st.files_done = done + i;
                drop(st);
                waker();
            });
            done += take.files.len();
            let mut st = s2.lock_ok();
            st.files_done = done;
            if ok {
                eprintln!("Recovered an unfinished take: {}", take.dir.display());
                st.rescued.push(take.dir.clone());
            }
        }
        s2.lock_ok().finished = true;
        waker();
    });
    if spawned.is_err() {
        // No thread: do it here instead.
        let mut st = state.lock_ok();
        st.rescued = folders.iter().flat_map(|f| recover_unfinished(f)).collect();
        st.takes = st.rescued.clone();
        st.scanning = false;
        st.finished = true;
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::wav::{SampleFormat, WavWriter};

    #[test]
    fn repairs_and_marks_take() {
        let root = std::env::temp_dir().join(format!("cutaway-recovery-{}", std::process::id()));
        let take = root.join("Show_2026-01-01_00-00-00");
        fs::create_dir_all(&take).unwrap();
        // A WAV whose writer never finished.
        let wav_path = take.join("Audio_01_DAW.wav");
        let mut w = WavWriter::create(&wav_path, SampleFormat::from_bits(24), 2, 48000).unwrap();
        // Over a second, so some of it reached the disk.
        w.write(&vec![0.25f32; 2 * 48_000 * 2 + 9600]).unwrap();
        std::mem::forget(w); // simulate a crash: no finish()
        // Something that isn't ours is left alone.
        fs::create_dir_all(root.join("Other")).unwrap();
        let done = recover_unfinished(&root);
        assert_eq!(done, vec![take.clone()]);
        let note = fs::read_to_string(take.join("Take.txt")).unwrap();
        assert!(note.contains("Audio_01_DAW.wav"), "{note}");
        // When and how long, for Record & stream › Recordings and uploads.
        assert!(note.contains("Recorded: 2026-01-01 00:00\nLength: 00:00:0"), "{note}");
        assert!(note.contains("  Audio_01_DAW.wav  ("), "listed with its size: {note}");
        assert!(recover_unfinished(&root).is_empty(), "second pass leaves finished recordings alone");
        assert_eq!(stamp_of("Show_2026-11-01_20-04-12_2").map(|t| t.to_string()), Some("2026-11-01 20:04:12".into()));
        assert_eq!(stamp_of("My_Show_2026-11-01_20-04-12").map(|t| t.to_string()), Some("2026-11-01 20:04:12".into()));
        assert!(stamp_of("Other").is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remembers_where_takes_went() {
        let dir = std::env::temp_dir().join(format!("cutaway-folders-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(remembered_folders(&dir).is_empty());
        remember_folders(&dir, &[PathBuf::from("/a"), PathBuf::from("/backup")]);
        remember_folders(&dir, &[PathBuf::from("/b")]);
        remember_folders(&dir, &[PathBuf::from("/a")]);
        assert_eq!(remembered_folders(&dir), vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/backup")]);
        for i in 0..20 {
            remember_folders(&dir, &[PathBuf::from(format!("/{i}"))]);
        }
        assert_eq!(remembered_folders(&dir).len(), REMEMBERED);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rescues_in_the_background() {
        let root = std::env::temp_dir().join(format!("cutaway-rescue-{}", std::process::id()));
        let take = root.join("Show_2026-01-02_00-00-00");
        fs::create_dir_all(&take).unwrap();
        for name in ["Audio_01_A.wav", "Audio_02_B.wav"] {
            let mut w = WavWriter::create(&take.join(name), SampleFormat::from_bits(16), 1, 48000).unwrap();
            w.write(&vec![0.1f32; 4800]).unwrap();
            std::mem::forget(w);
        }
        let none = start_rescue(&[root.join("nothing here")], Arc::new(|| {}));
        let t0 = std::time::Instant::now();
        while none.lock_ok().scanning && t0.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(none.lock_ok().takes.is_empty(), "nothing to rescue");
        let st = start_rescue(&[root.clone()], Arc::new(|| {}));
        let t0 = std::time::Instant::now();
        while !st.lock_ok().finished && t0.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let st = st.lock_ok().clone();
        assert!(st.finished);
        assert_eq!(st.files_total, 2);
        assert_eq!(st.rescued, vec![take.clone()]);
        assert_eq!(st.fraction(), 1.0);
        assert!(take.join("Take.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
