//! A problem report: one .zip with what's needed to find out what went
//! wrong (the log, the last take's note, the settings and this computer's
//! devices), with stream keys and passwords taken out.

use crate::config::Config;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The most of each log to include (the end of it).
const LOG_TAIL: usize = 4_000_000;

pub struct Report<'a> {
    pub cfg: &'a Config,
    /// Version, engine and similar facts, as shown in Diagnostics.
    pub facts: &'a [(String, String)],
    /// Cameras, microphones and other devices found.
    pub devices: &'a [String],
    pub log: PathBuf,
}

/// The user's home folder as it appears in paths (both slash styles on
/// Windows), longest first.
fn dirs_home() -> Vec<String> {
    let Some(h) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) else { return Vec::new() };
    let h = h.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    if h.len() < 4 {
        return Vec::new();
    }
    let mut v = vec![h.clone()];
    if cfg!(windows) {
        v.push(h.replace('\\', "/"));
        v.push(h.replace('\\', "\\\\"));
    }
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    v
}

/// SRT passphrases (`passphrase=…` in an address) hidden, wherever they are.
fn hide_passphrases(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("passphrase=") {
        let (before, after) = rest.split_at(i + "passphrase=".len());
        out.push_str(before);
        let end = after.find(|c: char| c == '&' || c == '"' || c == '\'' || c.is_whitespace()).unwrap_or(after.len());
        if end > 0 {
            out.push_str("[hidden]");
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// A suggested file name: "Cutaway problem report 2026-10-25 1930.zip".
pub fn file_name() -> String {
    format!("Cutaway problem report {}.zip", chrono::Local::now().format("%Y-%m-%d %H%M"))
}

impl Report<'_> {
    pub fn write(&self, path: &Path) -> Result<()> {
        // Keys and passwords, and the remote's PIN (it lets anyone on the
        // network run the show).
        // Stream keys of RTMP inputs (`rtmp-in:1935/<key>`) too.
        let secrets: Vec<String> = {
            let mut c = self.cfg.clone();
            let mut v: Vec<String> = c.secrets_mut().into_iter().map(|(_, v)| v.trim().to_string()).collect();
            v.push(self.cfg.remote.pin.trim().to_string());
            for src in &self.cfg.sources {
                if let Some(key) = crate::util::strip_prefix_ci(src.device.trim(), "rtmp-in:").and_then(|r| r.split_once('/')).map(|(_, k)| k.trim()) {
                    v.push(key.to_string());
                }
            }
            // As typed, and as they'd appear in a web address.
            let encoded: Vec<String> = v.iter().map(|s| crate::net::google::encode(s)).filter(|e| !v.contains(e)).collect();
            v.extend(encoded);
            let mut v: Vec<String> = v.into_iter().filter(|v| v.len() >= 4).collect();
            // Longest first, so a key inside a longer one is hidden whole.
            v.sort_by_key(|s| std::cmp::Reverse(s.len()));
            v
        };
        // Who this is: the signed-in Google account, and the user's own
        // folder (which carries their name) in every path.
        let account = self.cfg.upload.account.trim().to_string();
        let home = dirs_home();
        let scrub = |text: &str| -> String {
            let mut t = text.to_string();
            for s in &secrets {
                t = t.replace(s.as_str(), "[hidden]");
            }
            if account.len() >= 3 {
                t = t.replace(account.as_str(), "[account]");
            }
            for h in &home {
                t = t.replace(h.as_str(), "~");
            }
            hide_passphrases(&t)
        };

        let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, text: &str| -> Result<()> {
            zip.start_file(name, opts)?;
            zip.write_all(text.as_bytes())?;
            Ok(())
        };

        add("About this computer.txt", &self.about())?;
        let mut shared = self.cfg.without_secrets();
        shared.remote.pin.clear();
        let settings = toml::to_string_pretty(&shared)?;
        add("Settings (no keys or passwords).toml", &scrub(&settings))?;
        for (name, p) in [("Cutaway.log", self.log.clone()), ("Cutaway.previous.log", self.log.with_extension("previous.log"))] {
            if let Some(text) = tail(&p, LOG_TAIL) {
                add(name, &scrub(&text))?;
            }
        }
        if let Some((dir, note)) = last_take(&self.cfg.output.folder) {
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            add(&format!("Last recording ({name}).txt"), &scrub(&note))?;
        }
        zip.finish()?;
        Ok(())
    }

    fn about(&self) -> String {
        let mut s = format!(
            "Cutaway {} on {} {}\nMade {}\n\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        );
        for (k, v) in self.facts {
            s.push_str(&format!("{k}: {v}\n"));
        }
        s.push_str("\nDevices\n");
        for d in self.devices {
            s.push_str(&format!("  {d}\n"));
        }
        s
    }
}

/// The end of a text file, starting at a whole line.
fn tail(path: &Path, max: usize) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() <= max {
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    let cut = &bytes[bytes.len() - max..];
    let start = cut.iter().position(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
    Some(format!("(earlier lines left out)\n{}", String::from_utf8_lossy(&cut[start..])))
}

/// The newest take's folder and its note (Take.txt).
fn last_take(output: &Path) -> Option<(PathBuf, String)> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(output).ok()?.flatten() {
        let note = e.path().join("Take.txt");
        if let Ok(m) = std::fs::metadata(&note).and_then(|m| m.modified()) {
            if newest.as_ref().is_none_or(|(t, _)| m > *t) {
                newest = Some((m, e.path()));
            }
        }
    }
    let (_, dir) = newest?;
    let note = std::fs::read_to_string(dir.join("Take.txt")).ok()?;
    Some((dir, note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceConfig;
    use std::io::Read;

    #[test]
    fn report_has_no_keys() {
        let dir = std::env::temp_dir().join(format!("cutaway-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let take = dir.join("Takes").join("2026-10-25 1900");
        std::fs::create_dir_all(&take).unwrap();
        std::fs::write(take.join("Take.txt"), "Program.mp4: 10 minutes").unwrap();
        let log = dir.join("Cutaway.log");
        std::fs::write(&log, "connecting to rtmp://a.rtmp.youtube.com/live2/live_secret_123\n").unwrap();

        let mut cfg = Config::default();
        cfg.output.folder = dir.join("Takes");
        cfg.streaming.destinations.push(Default::default());
        cfg.streaming.destinations[0].key = "live_secret_123".into();
        cfg.guests.turn_password = "hunter22".into();
        cfg.remote.pin = "975318".into();
        cfg.upload.account = "maya@example.com".into();
        cfg.sources.push(SourceConfig { device: "rtmp-in:1935/cam-Zq8rT2vX9kPw".into(), ..Default::default() });
        cfg.sources.push(SourceConfig { device: "srt-in:9000?passphrase=Sup3rSecretPass".into(), ..Default::default() });
        std::fs::write(&log, "connecting to rtmp://a.rtmp.youtube.com/live2/live_secret_123\nsigned in as maya@example.com\n").unwrap();

        let out = dir.join("report.zip");
        Report { cfg: &cfg, facts: &[("Engine".into(), "Metal".into())], devices: &["FaceTime camera".into()], log }
            .write(&out)
            .unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut all = String::new();
        let mut names = Vec::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            names.push(f.name().unwrap().to_string());
            f.read_to_string(&mut all).unwrap();
        }
        assert!(names.iter().any(|n| n.starts_with("Last recording")), "{names:?}");
        assert!(all.contains("Program.mp4: 10 minutes"));
        assert!(all.contains("FaceTime camera"));
        assert!(all.contains("live2/[hidden]"));
        assert!(!all.contains("live_secret_123"));
        assert!(!all.contains("975318"));
        assert!(!all.contains("hunter22"));
        assert!(!all.contains("cam-Zq8rT2vX9kPw") && all.contains("rtmp-in:1935/[hidden]"), "the RTMP input's key");
        assert!(!all.contains("maya@example.com") && all.contains("[account]"));
        assert!(!all.contains("Sup3rSecretPass") && all.contains("passphrase=[hidden]"), "SRT passphrases");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
