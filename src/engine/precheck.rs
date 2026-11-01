//! The pre-show check: everything that can be tested without going live.

use crate::util::LockExt;
use super::{encode, health, recorder, take, Engine};
use crate::config::StreamDestination;
use crossbeam_channel::{unbounded, Receiver};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Clone, Debug)]
pub struct CheckItem {
    pub level: Level,
    pub what: String,
    pub detail: String,
}

/// The check for the NDI runtime, when the show uses NDI.
pub const NDI: &str = "NDI";

fn item(level: Level, what: impl Into<String>, detail: impl Into<String>) -> CheckItem {
    CheckItem { level, what: what.into(), detail: detail.into() }
}

impl Engine {
    /// Checks that run instantly on this computer.
    pub fn local_checks(&self) -> Vec<CheckItem> {
        let mut out = Vec::new();
        let cfg = self.config();
        // Pictures.
        for (i, s) in self.sources.iter().enumerate() {
            let what = format!("Video: {}", s.name);
            // Offline as everywhere else; or nothing has come at all yet.
            if self.source_offline(i) || s.preview.latest().is_none() {
                let d = s.input.net.lock_ok().detail.clone();
                out.push(item(Level::Fail, what, if d.is_empty() { "no picture".to_string() } else { format!("no picture ({d})") }));
                continue;
            }
            // A black picture usually means a lens cap or a sleeping camera.
            let luma = s.preview.latest().map(|(_, f)| {
                let px = f.rgba.chunks_exact(4).step_by(7);
                let (sum, n) = px.fold((0.0f64, 0u32), |(a, n), p| (a + 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64, n + 1));
                sum / n.max(1) as f64 / 255.0
            });
            match luma {
                Some(l) if l < 0.03 => out.push(item(Level::Warn, what, "the picture is black (lens cap? camera asleep?)")),
                _ => out.push(item(Level::Ok, what, s.description())),
            }
        }
        // Sound.
        for (i, a) in self.audio.iter().enumerate() {
            let what = format!("Audio: {}", a.name);
            if self.input_offline(i) {
                out.push(item(Level::Fail, what, "no signal from the device"));
                continue;
            }
            let peak = a.shared.levels_db().into_iter().fold(-120.0f32, f32::max);
            if !a.shared.is_on() {
                out.push(item(Level::Warn, what, "turned off in the mix"));
            } else if peak < -60.0 && !a.input.from_source() {
                out.push(item(Level::Warn, what, "silent right now (muted at the source, or nobody talking?)"));
            } else {
                out.push(item(Level::Ok, what, format!("{} · level {:.0} dB", a.description(), peak.max(-90.0))));
            }
        }
        // NDI, if the show uses it.
        let ndi_sources = cfg.sources.iter().any(|s| s.device.trim().to_lowercase().starts_with("ndi:"));
        if ndi_sources || cfg.ndi_out.program || cfg.ndi_out.sources {
            match super::ndi::api_ready() {
                Ok(()) => out.push(item(Level::Ok, NDI, "installed")),
                Err(_) => out.push(item(Level::Fail, NDI, "isn't installed on this computer, and this show uses it (install it below)")),
            }
        }
        // Recording space.
        match self.disk_status() {
            Some(d) => match take::space_problem(&d) {
                Some(p) => out.push(item(Level::Fail, "Recording drive", p)),
                None => {
                    let level = if d.low() { Level::Warn } else { Level::Ok };
                    out.push(item(level, "Recording drive", d.describe()));
                }
            },
            None => out.push(item(Level::Warn, "Recording drive", format!("can't read the free space of {}", cfg.output.folder.display()))),
        }
        if let Some(b) = cfg.output.backup_folder.as_ref().filter(|p| !p.as_os_str().is_empty()) {
            let probe = b.join(".cutaway-write-test");
            let writable = std::fs::create_dir_all(b).and_then(|_| std::fs::write(&probe, b"ok")).is_ok();
            let _ = std::fs::remove_file(&probe);
            match (writable, health::disk_space(b)) {
                (false, _) => out.push(item(Level::Fail, "Backup drive", format!("can't write to {} (is the drive connected?)", b.display()))),
                (true, Some((free, _))) if free < health::MIN_START_BYTES => out.push(item(Level::Warn, "Backup drive", format!("only {} free", crate::util::human_size(free)))),
                (true, Some((free, _))) => out.push(item(Level::Ok, "Backup drive", format!("{} free", crate::util::human_size(free)))),
                (true, None) => out.push(item(Level::Ok, "Backup drive", "writable")),
            }
        }
        // The encoder starts.
        let codec = cfg.output.codec.video_codec();
        let enc = encode::resolve_backend(codec, &cfg.output.encoder).and_then(|backend| {
            let (w, h) = self.info.canvas;
            let mut s = recorder::video_settings(codec, self.info.fps, encode::record_bitrate_kbps(codec, 0, w, h, self.info.fps), backend);
            s.width = w;
            s.height = h;
            let mut e = encode::create_video(&s, std::sync::Arc::new(|_| {}))?;
            e.finish()
        });
        match enc {
            Ok(()) => out.push(item(Level::Ok, "Recording encoder", self.encoder_description())),
            Err(e) => out.push(item(Level::Fail, "Recording encoder", format!("{e:#}"))),
        }
        if let Some(f) = &self.status.lock_ok().fatal {
            out.push(item(Level::Fail, "Compositor", f.clone()));
        }
        out
    }
}

/// Contact each enabled stream destination without going live. Results
/// arrive on the channel as they come in.
pub fn check_destinations(dests: Vec<StreamDestination>) -> Receiver<CheckItem> {
    let (tx, rx) = unbounded();
    for d in dests.into_iter().filter(|d| d.enabled) {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let what = format!("Stream: {}", d.name);
            if let Some(p) = d.problem() {
                let _ = tx.send(item(Level::Fail, what, p));
                return;
            }
            let r = if d.service.is_srt() {
                crate::net::srt::SrtSender::connect(&d.url(), Duration::from_secs(4)).map(|s| s.close())
            } else {
                crate::net::rtmp::RtmpPublisher::probe(&d.url(), Duration::from_secs(5))
            };
            let _ = tx.send(match r {
                Ok(()) => item(Level::Ok, what, "the server answers"),
                Err(e) => item(Level::Fail, what, format!("{e:#}")),
            });
        });
    }
    rx
}
