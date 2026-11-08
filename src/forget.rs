//! More › App › Diagnostics › Remove my settings and keys: everything
//! Cutaway keeps about you on this computer goes (settings, show profiles,
//! saved keys and passwords, the log, the window's place, the uploads
//! still waiting), once Cutaway has closed. Recordings stay where they are.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static WANTED: AtomicBool = AtomicBool::new(false);
/// The settings file in use (--config may put it somewhere else) and the
/// recording folders, which are never touched.
static PLAN: Mutex<Option<(Option<PathBuf>, Vec<PathBuf>)>> = Mutex::new(None);

/// Remove everything once the window has closed (after the last save, so
/// nothing is written again behind it).
pub fn on_exit(config_path: Option<PathBuf>, keep: Vec<PathBuf>) {
    *PLAN.lock().unwrap_or_else(|e| e.into_inner()) = Some((config_path, keep));
    WANTED.store(true, Ordering::SeqCst);
}

pub fn wanted() -> bool {
    WANTED.load(Ordering::SeqCst)
}

/// Do it (after the window has closed). What couldn't be removed, if
/// anything, is returned (and printed).
pub fn run_if_wanted() -> Vec<String> {
    if !wanted() {
        return Vec::new();
    }
    let (config_path, keep) = PLAN.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let settings_dir = crate::config::default_config_path().parent().map(Path::to_path_buf);
    let logs_dir = crate::logging::path().parent().map(Path::to_path_buf);
    let problems = remove_all(settings_dir.as_deref(), config_path.as_deref(), logs_dir.as_deref(), &keep, &|| crate::secrets::forget_all());
    for p in &problems {
        eprintln!("remove my data: {p}");
    }
    problems
}

/// A folder of Cutaway's own: named Cutaway, so nothing else is ever
/// emptied (whatever the environment says the settings folder is).
fn ours(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("cutaway"))
        || (dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("logs")) && dir.parent().is_some_and(ours))
}

/// `p` is, or holds, one of the `keep` folders.
fn holds_kept(p: &Path, keep: &[PathBuf]) -> bool {
    keep.iter().any(|k| k.starts_with(p))
}

fn remove(p: &Path, keep: &[PathBuf], problems: &mut Vec<String>) {
    if holds_kept(p, keep) {
        // A recording folder inside: go through it, leaving it alone.
        if p.is_dir() && !keep.iter().any(|k| k == p) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    remove(&e.path(), keep, problems);
                }
            }
        }
        return;
    }
    // (symlink_metadata: a link is removed, never what it points at.)
    let Ok(meta) = std::fs::symlink_metadata(p) else { return };
    let r = if meta.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    if let Err(e) = r {
        // A log still open (Windows, where the log is standard error) is emptied instead.
        let emptied = meta.is_file() && std::fs::OpenOptions::new().write(true).open(p).and_then(|f| f.set_len(0)).is_ok();
        if !emptied {
            problems.push(format!("couldn't remove {}: {e}", p.display()));
        }
    }
}

fn remove_all(settings_dir: Option<&Path>, config_path: Option<&Path>, logs_dir: Option<&Path>, keep: &[PathBuf], forget_keys: &dyn Fn() -> Result<(), String>) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(e) = forget_keys() {
        problems.push(format!("couldn't remove the saved keys and passwords: {e}"));
    }
    // A settings file of its own (--config): that file, its show profiles
    // and its uploads list, and nothing else in that folder.
    if let Some(c) = config_path.filter(|c| settings_dir.is_none_or(|d| c.parent() != Some(d))) {
        remove(c, keep, &mut problems);
        remove(&c.with_extension("toml.saving"), keep, &mut problems);
        remove(&crate::config::profiles_dir(c), keep, &mut problems);
        if let Some(d) = c.parent() {
            remove(&d.join("uploads.json"), keep, &mut problems);
        }
    }
    for dir in [settings_dir, logs_dir].into_iter().flatten().filter(|d| ours(d)) {
        remove(dir, keep, &mut problems);
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cutaway-forget-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn everything_goes_but_recordings() {
        let d = temp("all");
        let settings = d.join("Cutaway");
        std::fs::create_dir_all(settings.join("profiles")).unwrap();
        std::fs::write(settings.join("config.toml"), "x").unwrap();
        std::fs::write(settings.join("profiles").join("Show.toml"), "x").unwrap();
        std::fs::write(settings.join("Main window.ron"), "x").unwrap();
        // Someone's recordings kept in the settings folder stay.
        let rec = settings.join("Recordings");
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(rec.join("Take 1.mov"), "x").unwrap();
        let logs = d.join("Logs").join("Cutaway");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("Cutaway.log"), "x").unwrap();
        let forgot = std::cell::Cell::new(false);
        let problems = remove_all(Some(&settings), Some(&settings.join("config.toml")), Some(&logs), std::slice::from_ref(&rec), &|| {
            forgot.set(true);
            Ok(())
        });
        assert!(problems.is_empty(), "{problems:?}");
        assert!(forgot.get());
        assert!(!settings.join("config.toml").exists());
        assert!(!settings.join("profiles").exists());
        assert!(!settings.join("Main window.ron").exists());
        assert!(rec.join("Take 1.mov").exists());
        assert!(!logs.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_settings_file_elsewhere_takes_only_its_own_files() {
        let d = temp("elsewhere");
        let mine = d.join("My shows");
        std::fs::create_dir_all(mine.join("profiles")).unwrap();
        std::fs::write(mine.join("studio.toml"), "x").unwrap();
        std::fs::write(mine.join("notes.txt"), "mine").unwrap();
        std::fs::write(mine.join("uploads.json"), "[]").unwrap();
        // A folder not named Cutaway is never emptied, even if it's where
        // the settings folder is said to be.
        let odd = d.join("Documents");
        std::fs::create_dir_all(&odd).unwrap();
        std::fs::write(odd.join("letter.txt"), "x").unwrap();
        let problems = remove_all(Some(&odd), Some(&mine.join("studio.toml")), None, &[], &|| Ok(()));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!mine.join("studio.toml").exists());
        assert!(!mine.join("profiles").exists());
        assert!(!mine.join("uploads.json").exists());
        assert!(mine.join("notes.txt").exists());
        assert!(odd.join("letter.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_removed_not_followed() {
        let d = temp("links");
        let settings = d.join("cutaway");
        std::fs::create_dir_all(&settings).unwrap();
        let elsewhere = d.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("keep.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, settings.join("link")).unwrap();
        let problems = remove_all(Some(&settings), None, None, &[], &|| Ok(()));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(elsewhere.join("keep.txt").exists());
        assert!(!settings.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_key_removal_is_said() {
        let problems = remove_all(None, None, None, &[], &|| Err("no".into()));
        assert_eq!(problems.len(), 1);
    }
}
