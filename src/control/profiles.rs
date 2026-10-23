//! Show profiles: complete setups (sources, scenes, audio, graphics,
//! streaming...) saved under a name, to switch between shows in one click.

use crate::config::Config;
use crate::util::{safe_file_name, LockExt};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

/// The saved shows: (name, file). The real name is inside each file (file
/// names are made safe, so two names can share a safe form).
/// Files are only read again when they change (menus list the shows on
/// every frame they're open).
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    type Files = Vec<(PathBuf, Option<std::time::SystemTime>, u64)>;
    static CACHE: std::sync::Mutex<Option<(PathBuf, Files, Vec<(String, PathBuf)>)>> = std::sync::Mutex::new(None);
    let Ok(d) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Files = d
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .map(|e| {
            let m = e.metadata().ok();
            (e.path(), m.as_ref().and_then(|m| m.modified().ok()), m.map_or(0, |m| m.len()))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut cache = CACHE.lock_ok();
    if let Some((d, f, e)) = cache.as_ref() {
        if d == dir && *f == files {
            return e.clone();
        }
    }
    let found: Vec<(String, PathBuf)> = files
        .iter()
        .filter_map(|(p, _, _)| {
            let named = Config::load(p).ok().map(|c| c.ui.profile).filter(|n| !n.trim().is_empty());
            let name = named.or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))?;
            Some((name, p.clone()))
        })
        .collect();
    *cache = Some((dir.to_path_buf(), files, found.clone()));
    found
}


/// The file the show `name` is saved in.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    entries(dir).into_iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name.trim())).map(|(_, p)| p)
}

/// A file for a new show, not one another show uses.
fn new_file(dir: &Path, name: &str) -> PathBuf {
    let base = safe_file_name(name.trim());
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{base}.toml") } else { format!("{base}_{n}.toml") }))
        .find(|p| !p.exists())
        .expect("a free file name")
}

/// Saved profiles, by name, sorted.
pub fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = entries(dir).into_iter().map(|(n, _)| n).collect();
    v.sort_by_key(|s| s.to_lowercase());
    v.dedup();
    v
}

/// Save `cfg` as profile `name` (replacing one with that name).
pub fn save(dir: &Path, name: &str, cfg: &Config) -> Result<()> {
    if name.trim().is_empty() {
        bail!("give the show a name");
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut c = cfg.clone();
    c.ui.profile = name.trim().to_string();
    c.save(&find(dir, name).unwrap_or_else(|| new_file(dir, name)))
}

pub fn load(dir: &Path, name: &str) -> Result<Config> {
    let p = find(dir, name).ok_or_else(|| anyhow!("there's no show called \"{name}\""))?;
    let mut c = Config::load(&p).with_context(|| format!("loading the show \"{name}\""))?;
    c.ui.profile = name.trim().to_string();
    Ok(c)
}

pub fn delete(dir: &Path, name: &str) -> Result<()> {
    let p = find(dir, name).ok_or_else(|| anyhow!("there's no show called \"{name}\""))?;
    std::fs::remove_file(p).with_context(|| format!("deleting the show \"{name}\""))
}

pub fn rename(dir: &Path, from: &str, to: &str) -> Result<()> {
    if to.trim().is_empty() {
        bail!("give the show a name");
    }
    if list(dir).iter().any(|n| n.eq_ignore_ascii_case(to.trim())) {
        bail!("there's already a show called \"{to}\"");
    }
    let old = find(dir, from).ok_or_else(|| anyhow!("there's no show called \"{from}\""))?;
    let c = load(dir, from)?;
    // A new file first (even when both names have the same safe form), then
    // the old one goes.
    save(dir, to, &c)?;
    std::fs::remove_file(old).with_context(|| format!("removing the old show \"{from}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_load_rename_delete() {
        let dir = std::env::temp_dir().join(format!("cutaway-profiles-{}", std::process::id()));
        let mut cfg = Config::default();
        cfg.program.fps = 25;
        save(&dir, "Sunday Service", &cfg).unwrap();
        save(&dir, "Podcast: ep/1", &Config::default()).unwrap();
        assert_eq!(list(&dir), vec!["Podcast: ep/1".to_string(), "Sunday Service".to_string()]);
        let back = load(&dir, "Sunday Service").unwrap();
        assert_eq!(back.program.fps, 25);
        assert_eq!(back.ui.profile, "Sunday Service");
        rename(&dir, "Sunday Service", "Church").unwrap();
        assert_eq!(list(&dir), vec!["Church".to_string(), "Podcast: ep/1".to_string()]);
        delete(&dir, "Church").unwrap();
        assert_eq!(list(&dir).len(), 1);
        // Names with the same safe file name stay apart, and renaming to one
        // keeps the show.
        save(&dir, "Podcast_ ep_1", &Config::default()).unwrap();
        assert_eq!(list(&dir).len(), 2);
        rename(&dir, "Podcast_ ep_1", "Podcast  ep 1").unwrap();
        assert_eq!(list(&dir), vec!["Podcast  ep 1".to_string(), "Podcast: ep/1".to_string()]);
        assert!(load(&dir, "Podcast  ep 1").is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
