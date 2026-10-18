//! Capture devices: what's connected and what it can do.

use anyhow::{anyhow, Result};

pub use crate::media::testsrc::PATTERNS as TEST_PATTERN_NAMES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

/// A capture mode a video device offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub fps: Vec<u32>,
    /// Compressed on the wire (decoded automatically).
    pub mjpeg_only: bool,
}

impl Mode {
    pub fn label(&self) -> String {
        let rates: Vec<String> = self.fps.iter().map(|f| f.to_string()).collect();
        format!("{}×{}  @ {} fps{}", self.width, self.height, rates.join("/"), if self.mjpeg_only { "  (MJPEG)" } else { "" })
    }
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    /// Platform identifier (stable across reconnects).
    pub id: String,
    /// Short, human readable list of supported modes.
    pub summary: String,
    pub max_channels: u32,
    /// Sample rate the device runs at (audio).
    #[allow(dead_code)]
    pub rate: u32,
    pub modes: Vec<Mode>,
}

/// List capture devices of one kind.
pub fn list(kind: Kind) -> Vec<DeviceInfo> {
    let found = crate::util::guard("Listing devices", || {
        Ok(match kind {
            Kind::Video => crate::platform::list_video_devices(),
            Kind::Audio => crate::platform::list_audio_devices(),
        })
    });
    found.unwrap_or_else(|e| {
        eprintln!("{e:#}");
        Vec::new()
    })
}

/// Match a device spec from the settings: exact name, partial name
/// (case-insensitive) or "#N" (1-based).
pub fn find<'a>(spec: &str, devices: &'a [DeviceInfo]) -> Result<&'a DeviceInfo> {
    let spec = spec.trim();
    let found = if let Some(n) = spec.strip_prefix('#') {
        n.trim().parse::<usize>().ok().and_then(|i| i.checked_sub(1)).and_then(|i| devices.get(i))
    } else {
        let want = spec.to_lowercase();
        devices
            .iter()
            .find(|d| d.name.to_lowercase() == want || d.id == spec)
            .or_else(|| devices.iter().find(|d| d.name.to_lowercase().contains(&want)))
    };
    found.ok_or_else(|| {
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        anyhow!("no device matches \"{spec}\". Found: {}", if names.is_empty() { "none".into() } else { names.join(", ") })
    })
}

/// Summarise modes for display: "1920×1080@30/60, 1280×720@60 (+3 more)".
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn summarize(modes: &[Mode]) -> String {
    let mut parts: Vec<String> = modes
        .iter()
        .map(|m| {
            let rates: Vec<String> = m.fps.iter().map(|f| f.to_string()).collect();
            format!("{}×{}@{}", m.width, m.height, rates.join("/"))
        })
        .collect();
    let more = parts.len().saturating_sub(6);
    parts.truncate(6);
    let mut s = parts.join(", ");
    if more > 0 {
        s.push_str(&format!(" (+{more} more)"));
    }
    s
}

pub fn print_devices() {
    for (kind, title) in [(Kind::Video, "Video sources"), (Kind::Audio, "Audio inputs")] {
        println!("{title}:");
        let list = list(kind);
        if list.is_empty() {
            println!("  (none found)");
        }
        for (i, d) in list.iter().enumerate() {
            println!("  #{:<2} {}", i + 1, d.name);
            if !d.summary.is_empty() {
                println!("       {}", d.summary);
            }
        }
        println!();
    }
    println!("Also available: device = \"screen\" (screen capture) and device = \"test\" (test pattern / tone).");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo { name: name.into(), id: format!("id-{name}"), summary: String::new(), max_channels: 2, rate: 48000, modes: vec![] }
    }

    #[test]
    fn finding() {
        let list = vec![dev("FaceTime HD Camera"), dev("Cam Link 4K")];
        assert_eq!(find("cam link", &list).unwrap().name, "Cam Link 4K");
        assert_eq!(find("#1", &list).unwrap().name, "FaceTime HD Camera");
        assert_eq!(find("id-Cam Link 4K", &list).unwrap().name, "Cam Link 4K");
        assert!(find("#3", &list).is_err());
        assert!(find("Elgato", &list).is_err());
    }
}
