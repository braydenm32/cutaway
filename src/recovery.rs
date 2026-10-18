//! Repairs takes that were never stopped properly (crash, power cut, force
//! quit). Video files are written as fragments while recording, so
//! everything up to the last second is on disk; this turns them back into
//! ordinary MP4/MOV files and fixes the WAV headers.

use crate::media::{mp4, wav};
use std::fs;
use std::path::{Path, PathBuf};

fn is_media(p: &Path, exts: &[&str]) -> bool {
    p.extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Finds unfinished takes in `output` and repairs them. Returns their folders.
pub fn recover_unfinished(output: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(output) else { return Vec::new() };
    let mut repaired = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || dir.join("Take.txt").exists() {
            continue;
        }
        let Ok(files) = fs::read_dir(&dir) else { continue };
        let files: Vec<PathBuf> = files.flatten().map(|f| f.path()).collect();
        let ours = files.iter().any(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            (name.starts_with("Program.") || name.starts_with("ISO_") || name.starts_with("Audio_")) && is_media(p, &["mp4", "mov", "wav"])
        });
        if !ours {
            continue;
        }
        let mut lines = Vec::new();
        for p in &files {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if is_media(p, &["mp4", "mov"]) {
                match mp4::recover(p) {
                    Ok(mp4::Recovery::Recovered { seconds }) => lines.push(format!("  {name}: recovered {seconds:.1} s")),
                    Ok(mp4::Recovery::AlreadyComplete) => lines.push(format!("  {name}: complete")),
                    Ok(mp4::Recovery::NotRecoverable) => lines.push(format!("  {name}: nothing could be recovered")),
                    Err(e) => lines.push(format!("  {name}: couldn't repair ({e:#})")),
                }
            } else if is_media(p, &["wav"]) {
                match wav::repair(p) {
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
        if fs::write(dir.join("Take.txt"), note).is_ok() {
            repaired.push(dir);
        }
    }
    repaired
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
}
