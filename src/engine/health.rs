//! Disk space and recording health: how much recording time is left, and
//! whether every source, file and input is keeping up.

use crate::util::LockExt;
use super::inputs::{AudioInputDev, VideoInput};
use super::recorder::VideoStats;
use super::{LogLevel, Status};
use crate::config::Config;
use crate::engine::encode;
use crate::util::human_size;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Below this, a take won't start.
pub const MIN_START_BYTES: u64 = 2_000_000_000;
pub const MIN_START_SECONDS: f64 = 10.0 * 60.0;
/// Below this, a running take is stopped so its files stay intact.
const STOP_BYTES: u64 = 1_000_000_000;
const STOP_SECONDS: f64 = 120.0;

#[derive(Clone, Debug)]
pub struct DiskStatus {
    pub path: PathBuf,
    pub free: u64,
    pub total: u64,
    /// Bytes per second being (or that would be) written there.
    pub rate: f64,
    /// Measured from the take in progress rather than estimated.
    pub measured: bool,
}

impl DiskStatus {
    pub fn seconds_left(&self) -> Option<f64> {
        (self.rate > 1.0).then(|| self.free as f64 / self.rate)
    }

    /// "412 GB free · about 9 h 20 min"
    pub fn describe(&self) -> String {
        match self.seconds_left() {
            Some(s) => format!("{} free · about {} of recording", human_size(self.free), fmt_left(s)),
            None => format!("{} free", human_size(self.free)),
        }
    }
}

/// How long a take must run before its measured write rate is trusted over the settings' estimate.
pub const RATE_SETTLE_SECONDS: f64 = 30.0;

/// The one write-rate figure used for "time left": the measured rate once
/// the take has run long enough to settle, otherwise the estimate from the
/// settings. Returns the rate and whether it was measured.
pub fn pick_rate(measured: Option<f64>, running_for: f64, estimate: f64) -> (f64, bool) {
    match measured {
        Some(r) if running_for >= RATE_SETTLE_SECONDS => (r.max(estimate * 0.25), true),
        _ => (estimate, false),
    }
}

pub fn fmt_left(secs: f64) -> String {
    let m = (secs / 60.0) as u64;
    if m >= 600 {
        format!("{} h", m / 60)
    } else if m >= 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// Free and total bytes on the drive holding `path` (or its nearest
/// existing parent).
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(p.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let unit = st.f_frsize as u64;
        Some((st.f_bavail as u64 * unit, st.f_blocks as u64 * unit))
    }
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: plain call with valid out-pointers.
        unsafe { GetDiskFreeSpaceExW(&HSTRING::from(p.as_os_str()), Some(&raw mut free), Some(&raw mut total), None) }.ok()?;
        Some((free, total))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Bytes per second a take writes with these settings (main drive).
pub fn estimate_rate(cfg: &Config, wav_channels: &[(u32, bool)]) -> f64 {
    let out = &cfg.output;
    let codec = out.codec.video_codec();
    let p = &cfg.program;
    let mut kbps = encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, p.width, p.height, p.fps) as f64;
    let mut files_with_audio = 1.0;
    let proxy_kbps = 3000.0;
    if out.proxies {
        kbps += proxy_kbps;
        files_with_audio += 1.0;
    }
    for s in cfg.sources.iter().filter(|s| s.record) {
        kbps += encode::record_bitrate_kbps(codec, out.iso_bitrate_kbps, s.width, s.height, s.fps) as f64;
        if out.iso_audio {
            files_with_audio += if out.proxies { 2.0 } else { 1.0 };
        }
        if out.proxies {
            kbps += proxy_kbps;
        }
    }
    if cfg.vertical.enabled && cfg.vertical.record {
        let v = &cfg.vertical;
        kbps += encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, v.width, v.height, p.fps) as f64;
        files_with_audio += 1.0;
    }
    if !cfg.audio.is_empty() {
        kbps += out.audio_bitrate_kbps as f64 * files_with_audio;
        if out.input_tracks {
            // Each input's track, in the Program file (and its proxy).
            kbps += out.audio_bitrate_kbps as f64 * cfg.audio.len() as f64 * if out.proxies { 2.0 } else { 1.0 };
        }
    }
    let mut wav: f64 = wav_channels.iter().filter(|(_, rec)| *rec).map(|(ch, _)| 48_000.0 * *ch as f64 * out.wav_bit_depth as f64 / 8.0).sum();
    if (out.mix_wav || cfg.podcast.enabled) && !cfg.audio.is_empty() {
        wav += 48_000.0 * 2.0 * out.wav_bit_depth as f64 / 8.0;
    }
    kbps * 1000.0 / 8.0 + wav
}

#[derive(Clone, Debug, Default)]
pub struct SourceHealth {
    pub name: String,
    pub fps: f32,
    pub expected_fps: u32,
    pub alive: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FileHealth {
    pub label: String,
    pub encoded: u64,
    pub repeated: u64,
    pub dropped: u64,
    pub resized: u64,
    pub backlog: u64,
    pub failed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AudioHealth {
    pub name: String,
    pub alive: bool,
    pub resyncs: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Health {
    pub sources: Vec<SourceHealth>,
    pub files: Vec<FileHealth>,
    pub audio: Vec<AudioHealth>,
    /// Program frames the compositor had to skip (overload).
    pub compositor_skips: u64,
}

impl Health {
    /// Anything that needs attention, in words.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.sources {
            if !s.alive {
                out.push(format!("{}: no signal", s.name));
            } else if s.expected_fps > 0 && s.fps < s.expected_fps as f32 * 0.8 {
                out.push(format!("{}: {:.0} fps (set to {})", s.name, s.fps, s.expected_fps));
            }
        }
        for f in &self.files {
            if f.failed {
                out.push(format!("{}: failed", f.label));
            } else if f.backlog > 30 {
                out.push(format!("{}: encoder falling behind", f.label));
            }
        }
        for a in &self.audio {
            if !a.alive {
                out.push(format!("{}: no signal", a.name));
            }
        }
        out
    }
}

/// The take being recorded, as the monitor sees it.
pub struct TakeMonitor {
    pub folder: PathBuf,
    pub backup: Option<PathBuf>,
    pub stats: Vec<(String, Arc<VideoStats>)>,
    sizes: VecDeque<(Instant, u64)>,
    warned: u8,
    backup_warned: bool,
    started: Instant,
}

impl TakeMonitor {
    pub fn new(folder: PathBuf, backup: Option<PathBuf>, stats: Vec<(String, Arc<VideoStats>)>) -> Self {
        TakeMonitor { folder, backup, stats, sizes: VecDeque::new(), warned: 0, backup_warned: false, started: Instant::now() }
    }

    /// Bytes per second written, once there's enough history.
    fn measured_rate(&mut self) -> Option<f64> {
        let size = folder_size(&self.folder);
        let now = Instant::now();
        self.sizes.push_back((now, size));
        while self.sizes.len() > 1 && now.duration_since(self.sizes[0].0).as_secs_f64() > 10.0 {
            self.sizes.pop_front();
        }
        let (t, s) = *self.sizes.front()?;
        let dt = now.duration_since(t).as_secs_f64();
        (dt >= 4.0).then(|| (size.saturating_sub(s)) as f64 / dt)
    }
}

/// Everything in a take's folder, its Proxies, Clips and Guests folders
/// included.
pub fn folder_size(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: u32) -> u64 {
        let Ok(d) = std::fs::read_dir(dir) else { return 0 };
        d.flatten()
            .filter_map(|e| Some((e.path(), e.metadata().ok()?)))
            .map(|(p, m)| if m.is_dir() { if depth < 3 { walk(&p, depth + 1) } else { 0 } } else { m.len() })
            .sum()
    }
    walk(dir, 0)
}

/// Runs every second on the watchdog thread.
pub struct HealthMonitor {
    pub cfg: Arc<Mutex<Config>>,
    pub status: Arc<Mutex<Status>>,
    pub take: Arc<Mutex<Option<TakeMonitor>>>,
    /// The sources and inputs (they change when they're added or removed).
    pub lineup: super::SharedLineup,
    pub compositor_skips: Arc<AtomicU64>,
    pub last_frames: Vec<(Instant, u64)>,
    pub seen: u64,
}

impl HealthMonitor {
    pub fn tick(&mut self) {
        let cfg = self.cfg.lock_ok().clone();
        let (generation, sources, audio): (u64, Vec<Arc<VideoInput>>, Vec<Arc<AudioInputDev>>) = {
            let l = self.lineup.lock_ok();
            (l.generation, l.video.clone(), l.audio.clone())
        };
        let channels: Vec<(u32, bool)> = audio.iter().map(|a| (a.channels(), a.cfg.record)).collect();
        let estimate = estimate_rate(&cfg, &channels);

        // Sources: frame rate actually arriving.
        if self.last_frames.len() != sources.len() || self.seen != generation {
            self.seen = generation;
            self.last_frames = sources.iter().map(|s| (Instant::now(), s.slot.frames())).collect();
        }
        let mut health = Health { compositor_skips: self.compositor_skips.load(Ordering::Relaxed), ..Default::default() };
        for (s, last) in sources.iter().zip(self.last_frames.iter_mut()) {
            let frames = s.slot.frames();
            let dt = last.0.elapsed().as_secs_f32().max(0.1);
            let fps = (frames.saturating_sub(last.1)) as f32 / dt;
            *last = (Instant::now(), frames);
            health.sources.push(SourceHealth { name: s.cfg.name.clone(), fps, expected_fps: s.cfg.fps, alive: s.alive() });
        }
        for a in &audio {
            // Sound from a media player or network input comes and goes.
            health.audio.push(AudioHealth { name: a.cfg.name.clone(), alive: a.alive() || a.from_source(), resyncs: a.shared.resyncs() });
        }

        // The take: files and disk.
        let mut take = self.take.lock_ok();
        let mut stop_reason = None;
        let mut warnings = Vec::new();
        let (main_rate, measured) = match take.as_mut() {
            Some(t) => {
                for (label, s) in &t.stats {
                    health.files.push(FileHealth {
                        label: label.clone(),
                        encoded: s.encoded.load(Ordering::Relaxed),
                        repeated: s.repeated.load(Ordering::Relaxed),
                        dropped: s.dropped.load(Ordering::Relaxed),
                        resized: s.resized.load(Ordering::Relaxed),
                        backlog: s.backlog.load(Ordering::Relaxed),
                        failed: s.failed.load(Ordering::Relaxed),
                    });
                }
                let measured = t.measured_rate();
                pick_rate(measured, t.started.elapsed().as_secs_f64(), estimate)
            }
            None => (estimate, false),
        };
        // The drive the take is filling (the folder setting may have been
        // changed since it started).
        let folder = take.as_ref().map(|t| t.folder.clone()).unwrap_or_else(|| cfg.output.folder.clone());
        let disk = disk_space(&folder).map(|(free, total)| DiskStatus { path: folder.clone(), free, total, rate: main_rate, measured });
        let backup = cfg
            .output
            .backup_folder
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
            .and_then(|p| disk_space(p).map(|(free, total)| DiskStatus { path: p.clone(), free, total, rate: main_rate, measured }));
        if let (Some(t), Some(d)) = (take.as_mut(), &disk) {
            let left = d.seconds_left().unwrap_or(f64::MAX);
            if d.free < STOP_BYTES || left < STOP_SECONDS {
                stop_reason = Some(format!("the drive for {} is almost full", d.path.display()));
            } else if left < 10.0 * 60.0 && t.warned < 2 {
                t.warned = 2;
                warnings.push(format!("Only about {} of recording space left on {}", fmt_left(left), d.path.display()));
            } else if left < 30.0 * 60.0 && t.warned < 1 {
                t.warned = 1;
                warnings.push(format!("About {} of recording space left on {}", fmt_left(left), d.path.display()));
            }
            if let Some(b) = &backup {
                if t.backup.is_some() && !t.backup_warned && (b.free < STOP_BYTES || b.seconds_left().is_some_and(|s| s < 10.0 * 60.0)) {
                    t.backup_warned = true;
                    warnings.push(format!("The backup drive ({}) is almost full; backup copies stop when it fills. The main recording continues.", b.path.display()));
                }
            }
        }
        drop(take);
        let mut st = self.status.lock_ok();
        st.health = health;
        st.disk = disk;
        st.backup_disk = backup;
        for w in warnings {
            st.push(LogLevel::Warning, w.clone());
            st.alerts.push(w);
        }
        if let Some(r) = stop_reason {
            if st.rec_failed.is_none() {
                st.push(LogLevel::Error, format!("Recording stopped: {r}. The files are kept intact."));
                st.rec_failed = Some(format!("{r}; recording was stopped so the files stay intact"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_estimated_until_settled() {
        assert_eq!(pick_rate(Some(8e6), 10.0, 2e6), (2e6, false));
        assert_eq!(pick_rate(None, 100.0, 2e6), (2e6, false));
        assert_eq!(pick_rate(Some(8e6), 40.0, 2e6), (8e6, true));
        // A stalled write rate doesn't promise endless time.
        assert_eq!(pick_rate(Some(0.0), 40.0, 2e6), (5e5, true));
    }

    #[test]
    fn estimates_and_formats() {
        let mut cfg = Config::default();
        cfg.sources.push(crate::config::SourceConfig::default());
        cfg.audio.push(crate::config::AudioConfig { channels: 2, ..Default::default() });
        let r = estimate_rate(&cfg, &[(2, true)]);
        // Program + one ISO at ~12 Mbps each, AAC, 24-bit stereo WAV: ~3.3 MB/s.
        assert!(r > 3.0e6 && r < 3.8e6, "{r}");
        assert_eq!(fmt_left(45.0 * 60.0), "45 min");
        assert_eq!(fmt_left(125.0 * 60.0), "2 h 5 min");
        assert!(disk_space(Path::new("/tmp/does/not/exist/yet")).is_some_and(|(f, t)| f <= t && t > 0));
    }
}
