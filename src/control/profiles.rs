//! Show profiles: complete setups (sources, scenes, audio, graphics,
//! streaming...) saved under a name, to switch between shows in one click.

use crate::config::Config;
use crate::util::safe_file_name;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.toml", safe_file_name(name.trim())))
}

/// Saved profiles, by name, sorted.
pub fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .filter_map(|p| {
                    // The real name is inside (file names are made safe).
                    let named = Config::load(&p).ok().map(|c| c.ui.profile).filter(|n| !n.trim().is_empty());
                    named.or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
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
    c.save(&file(dir, name))
}

pub fn load(dir: &Path, name: &str) -> Result<Config> {
    let p = file(dir, name);
    let mut c = Config::load(&p).with_context(|| format!("loading the show \"{name}\""))?;
    c.ui.profile = name.trim().to_string();
    Ok(c)
}

pub fn delete(dir: &Path, name: &str) -> Result<()> {
    std::fs::remove_file(file(dir, name)).with_context(|| format!("deleting the show \"{name}\""))
}

pub fn rename(dir: &Path, from: &str, to: &str) -> Result<()> {
    if list(dir).iter().any(|n| n.eq_ignore_ascii_case(to.trim())) {
        bail!("there's already a show called \"{to}\"");
    }
    let c = load(dir, from)?;
    save(dir, to, &c)?;
    delete(dir, from)
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
        std::fs::remove_dir_all(&dir).ok();
    }
}
