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
    let mut folders = vec![cfg.output.folder.clone()];
    folders.extend(cfg.output.backup_folder.clone().filter(|p| !p.as_os_str().is_empty()));
    // The backup set to the same folder: look there once.
    let same = |a: &Path, b: &Path| a == b || fs::canonicalize(a).ok().is_some_and(|a| fs::canonicalize(b).ok() == Some(a));
    if folders.len() == 2 && same(&folders[0], &folders[1]) {
        folders.pop();
    }
    folders
}

/// A take that was never stopped properly, and its media files.
pub struct Unfinished {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
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
        let Ok(files) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = files.flatten().map(|f| f.path()).filter(|p| is_media(p, &["mp4", "mov", "wav"])).collect();
        let ours = files.iter().any(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("Program.") || name.starts_with("ISO_") || name.starts_with("Audio_")
        });
        if ours {
            files.sort();
            takes.push(Unfinished { dir, files });
        }
    }
    takes.sort_by(|a, b| a.dir.cmp(&b.dir));
    takes
}

/// Repairs one take's files, calling `on_file` with each file's index and
/// name before it's worked on, and marks the take finished (Take.txt).
pub fn repair(take: &Unfinished, mut on_file: impl FnMut(usize, &str)) -> bool {
    let mut lines = Vec::new();
    for (i, p) in take.files.iter().enumerate() {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        on_file(i, &name);
        // A damaged file mustn't stop the rest (or the rescue screen).
        if is_media(p, &["mp4", "mov"]) {
            match crate::util::guard("repairing a recording", || mp4::recover(p)) {
                Ok(mp4::Recovery::Recovered { seconds }) => lines.push(format!("  {name}: recovered {seconds:.1} s")),
                Ok(mp4::Recovery::AlreadyComplete) => lines.push(format!("  {name}: complete")),
                Ok(mp4::Recovery::NotRecoverable) => lines.push(format!("  {name}: nothing could be recovered")),
                Err(e) => lines.push(format!("  {name}: couldn't repair ({e:#})")),
            }
        } else {
            match crate::util::guard("repairing a recording", || wav::repair(p)) {
                Ok(true) => lines.push(format!("  {name}: header repaired")),
                Ok(false) => lines.push(format!("  {name}: complete")),
                Err(e) => lines.push(format!("  {name}: couldn't repair ({e:#})")),
            }
        }
    }
    let note = format!(
        "Cutaway take (recovered)\n\nThis take wasn't stopped normally (crash, power loss or force quit).\n\
         Files play up to about the last second that was written.\n\nFiles:\n{}\n",
        lines.join("\n")
    );
    fs::write(take.dir.join("Take.txt"), note).is_ok()
}

/// Finds unfinished takes in `output` and repairs them. Returns their folders.
pub fn recover_unfinished(output: &Path) -> Vec<PathBuf> {
    find_unfinished(output).into_iter().filter(|t| repair(t, |_, _| {})).map(|t| t.dir).collect()
}

/// How a rescue in the background is getting on.
#[derive(Clone, Default)]
pub struct RescueState {
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

/// Rescues unfinished takes in these folders in the background. None if
/// there's nothing to rescue.
pub fn start_rescue(folders: &[PathBuf], waker: crate::engine::Waker) -> Option<Arc<Mutex<RescueState>>> {
    let takes: Vec<Unfinished> = folders.iter().flat_map(|f| find_unfinished(f)).collect();
    if takes.is_empty() {
        return None;
    }
    let state = Arc::new(Mutex::new(RescueState {
        takes: takes.iter().map(|t| t.dir.clone()).collect(),
        files_total: takes.iter().map(|t| t.files.len()).sum(),
        ..Default::default()
    }));
    let s2 = state.clone();
    let spawned = std::thread::Builder::new().name("rescue".into()).spawn(move || {
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
        st.finished = true;
    }
    Some(state)
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
        w.write(&vec![0.25f32; 9600]).unwrap();
        std::mem::forget(w); // simulate a crash: no finish()
        // Something that isn't ours is left alone.
        fs::create_dir_all(root.join("Other")).unwrap();
        let done = recover_unfinished(&root);
        assert_eq!(done, vec![take.clone()]);
        let note = fs::read_to_string(take.join("Take.txt")).unwrap();
        assert!(note.contains("Audio_01_DAW.wav"), "{note}");
        assert!(recover_unfinished(&root).is_empty(), "second pass leaves finished takes alone");
        fs::remove_dir_all(&root).unwrap();
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
        assert!(start_rescue(&[root.join("nothing here")], Arc::new(|| {})).is_none());
        let st = start_rescue(&[root.clone()], Arc::new(|| {})).expect("one take to rescue");
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
