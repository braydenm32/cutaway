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

/// What this computer has plugged in, kept for the whole run and filled on
/// a worker thread: listing cameras can open each one (slow on Windows),
/// so no screen waits for it.
#[derive(Debug, Default)]
pub struct Catalog {
    pub video: Vec<DeviceInfo>,
    pub audio: Vec<DeviceInfo>,
    /// Sound outputs (headphones, talkback).
    pub outputs: Vec<DeviceInfo>,
    /// Video encoders found for H.264 and H.265, best first.
    pub h264: Vec<String>,
    pub h265: Vec<String>,
    /// Bumped on every refresh, so a reader knows when to copy again.
    pub generation: u64,
}

struct CatalogState {
    current: std::sync::Arc<Catalog>,
    loaded: bool,
    running: bool,
    again: bool,
    last: Option<std::time::Instant>,
}

/// Background refresh at most this often when something asks.
const CATALOG_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);

fn catalog_state() -> &'static std::sync::Mutex<CatalogState> {
    static S: std::sync::OnceLock<std::sync::Mutex<CatalogState>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(CatalogState { current: Default::default(), loaded: false, running: false, again: false, last: None }))
}

fn build_catalog(generation: u64) -> Catalog {
    // Media Foundation and WASAPI need COM on this thread.
    #[cfg(windows)]
    crate::platform::win::com();
    let encoders = |c: crate::config::Codec| crate::engine::encode::video_backends(c.video_codec()).into_iter().map(String::from).collect::<Vec<_>>();
    Catalog {
        video: list(Kind::Video),
        audio: list(Kind::Audio),
        outputs: crate::util::guard("Listing sound outputs", || Ok(crate::platform::list_output_devices())).unwrap_or_default(),
        h264: encoders(crate::config::Codec::H264),
        h265: encoders(crate::config::Codec::H265),
        generation,
    }
}

/// Look again in the background. A request made while a look is under way
/// makes it go round once more, so a camera plugged in meanwhile shows up.
pub fn refresh_catalog() {
    let Ok(mut s) = catalog_state().lock() else { return };
    if s.running {
        s.again = true;
        return;
    }
    s.running = true;
    drop(s);
    let spawned = std::thread::Builder::new().name("device-catalog".into()).spawn(|| loop {
        let generation = catalog_state().lock().map(|s| s.current.generation + 1).unwrap_or(1);
        let found = std::sync::Arc::new(build_catalog(generation));
        let Ok(mut s) = catalog_state().lock() else { return };
        s.current = found;
        s.loaded = true;
        s.last = Some(std::time::Instant::now());
        if s.again {
            s.again = false;
        } else {
            s.running = false;
            return;
        }
    });
    if spawned.is_err() {
        if let Ok(mut s) = catalog_state().lock() {
            s.running = false;
        }
    }
}

/// The devices last found (empty until the first look is done), starting a
/// background look when it has never run or is older than 30 s. Never waits.
pub fn catalog() -> std::sync::Arc<Catalog> {
    let (current, stale) = match catalog_state().lock() {
        Ok(s) => (s.current.clone(), !s.running && s.last.is_none_or(|t| t.elapsed() > CATALOG_MAX_AGE)),
        Err(_) => return Default::default(),
    };
    if stale {
        refresh_catalog();
    }
    current
}

/// The devices last found, without starting a look.
pub fn catalog_now() -> std::sync::Arc<Catalog> {
    catalog_state().lock().map(|s| s.current.clone()).unwrap_or_default()
}

/// True until the first look has finished, or while one is under way.
pub fn catalog_busy() -> bool {
    catalog_state().lock().map(|s| s.running || !s.loaded).unwrap_or(false)
}

/// How likely a sound input is the mic someone speaks into: a USB or
/// interface mic (2) before the computer's own (1), before virtual cables,
/// app loopbacks and a nearby phone (0). By name: that's all the systems
/// tell us.
pub fn mic_rank(name: &str) -> u8 {
    let n = name.to_lowercase();
    let virtual_ = ["blackhole", "loopback", "vb-audio", "vb-cable", "cable output", "voicemeeter", "soundflower", "stereo mix", "what u hear", "zoomaudio", "teams audio", "virtual", "iphone", "ipad", "aggregate", "multi-output", "monitor of"];
    let built_in = ["built-in", "builtin", "internal", "macbook", "imac", "mac mini", "mac studio", "realtek", "microphone array", "intel", "smart sound", "conexant", "synaptics", "cirrus", "default"];
    if virtual_.iter().any(|w| n.contains(w)) {
        0
    } else if built_in.iter().any(|w| n.contains(w)) {
        1
    } else {
        2
    }
}

/// The input a new show (or ＋ Add) should start with: the best ranked by
/// `mic_rank`, the first of equals (forensics guide #2: a laptop's own mic
/// isn't picked over a USB mic that's plugged in).
pub fn best_mic(devices: &[DeviceInfo]) -> Option<&DeviceInfo> {
    let best = devices.iter().map(|d| mic_rank(&d.name)).max()?;
    devices.iter().find(|d| mic_rank(&d.name) == best)
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

#[cfg(test)]
mod mic_tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo { name: name.into(), id: name.into(), summary: String::new(), max_channels: 1, rate: 48_000, modes: Vec::new() }
    }

    /// The USB mic, not the laptop's own, and never a loopback first.
    #[test]
    fn picks_the_usb_mic() {
        let mac = [dev("MacBook Pro Microphone"), dev("BlackHole 2ch"), dev("Yeti Stereo Microphone")];
        assert_eq!(best_mic(&mac).unwrap().name, "Yeti Stereo Microphone");
        let win = [dev("Microphone Array (Realtek(R) Audio)"), dev("Microphone (Shure MV7)"), dev("CABLE Output (VB-Audio Virtual Cable)")];
        assert_eq!(best_mic(&win).unwrap().name, "Microphone (Shure MV7)");
        let alone = [dev("iPhone Microphone"), dev("MacBook Air Microphone")];
        assert_eq!(best_mic(&alone).unwrap().name, "MacBook Air Microphone");
        assert_eq!(best_mic(&[dev("BlackHole 2ch")]).unwrap().name, "BlackHole 2ch");
        assert!(best_mic(&[]).is_none());
        assert_eq!(mic_rank("Scarlett 2i2 USB"), 2);
        assert_eq!(mic_rank("ASIO: Focusrite USB ASIO"), 2);
    }
}
