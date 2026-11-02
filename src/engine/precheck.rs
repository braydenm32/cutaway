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
        // Sound, at the level its meter shows.
        let levels = self.input_levels();
        for (i, a) in self.audio.iter().enumerate() {
            let what = format!("Audio: {}", a.name);
            if self.input_offline(i) {
                out.push(item(Level::Fail, what, "no signal from the device"));
                continue;
            }
            let peak = levels.get(i).copied().flatten().unwrap_or(-120.0);
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
        // The encoders are tried in the background (see `encoder_plan`);
        // how much they'll have to do is worked out here.
        if let Some(plan) = self.encoder_plan() {
            let o = &cfg.output;
            let hardware = encode::resolve_backend(o.codec.video_codec(), &o.encoder).is_ok_and(encode::is_hardware);
            if let Some(text) = size_verdict(&plan.jobs, o.proxies, hardware) {
                out.push(item(Level::Warn, "Show size", text));
            }
        }
        if let Some(f) = &self.status.lock_ok().fatal {
            out.push(item(Level::Fail, "Compositor", f.clone()));
        }
        out
    }
}

/// Every encoder a show will run at once: a file each for the program
/// (and its clean and vertical versions) and each recorded source, plus
/// the stream. A graphics card can run out of encoder sessions, so they
/// are all opened together, the way recording does.
#[derive(Clone, Debug)]
pub struct EncoderPlan {
    pub jobs: Vec<(String, encode::VideoSettings)>,
    /// What the recording uses, for the "all fine" line.
    pub description: String,
}

impl Engine {
    /// What to open for the encoder check, or None while a take or stream
    /// is already using the encoders (opening more then proves nothing).
    pub fn encoder_plan(&self) -> Option<EncoderPlan> {
        if self.is_recording() || self.is_streaming() {
            return None;
        }
        let cfg = self.config();
        let out = &cfg.output;
        let codec = out.codec.video_codec();
        let fps = self.info.fps;
        let backend = if encode::is_auto(&out.encoder) { "auto" } else { out.encoder.as_str() };
        let file = |w: u32, h: u32, fps: u32, kbps: u32| {
            let mut s = recorder::video_settings(codec, fps, encode::record_bitrate_kbps(codec, kbps, w, h, fps), backend);
            (s.width, s.height) = (w & !1, h & !1);
            s
        };
        let (cw, ch) = self.info.canvas;
        let mut jobs = vec![("Program".to_string(), file(cw, ch, fps, out.program_bitrate_kbps))];
        if out.clean_program {
            jobs.push(("Program (clean)".into(), file(cw, ch, fps, out.program_bitrate_kbps)));
        }
        if cfg.vertical.enabled && cfg.vertical.record {
            let (vw, vh) = cfg.vertical.size();
            jobs.push(("Program (vertical)".into(), file(vw, vh, fps, out.program_bitrate_kbps)));
        }
        for (i, s) in self.sources.iter().enumerate().filter(|(_, s)| s.record) {
            jobs.push((format!("ISO {} ({})", i + 1, s.name), file(s.size.0, s.size.1, s.fps.max(1), out.iso_bitrate_kbps)));
        }
        let st = &cfg.streaming;
        if st.destinations.iter().any(|d| d.enabled) {
            jobs.push((
                "Stream".into(),
                encode::VideoSettings {
                    codec: crate::media::VideoCodec::H264,
                    width: st.width & !1,
                    height: st.height & !1,
                    fps: st.fps.min(fps).max(1),
                    bitrate_kbps: st.video_bitrate_kbps,
                    purpose: encode::Purpose::Stream,
                    backend: if encode::is_auto(&st.encoder) { "auto".into() } else { st.encoder.clone() },
                },
            ));
        }
        Some(EncoderPlan { jobs, description: self.encoder_description() })
    }
}

/// A 1080p30 file's pixels a second: the unit "this show is N times that"
/// counts in.
const HD30: f64 = 1920.0 * 1080.0 * 30.0;
/// How many 1080p30 files' worth a graphics-card encoder is trusted with
/// before the check says it may be too much (a guess on the cautious side:
/// four 4K30 files, or sixteen 1080p30 ones).
const HARDWARE_LIMIT: f64 = 16.0;

/// A line when the show may be too much for this computer: every file
/// (and proxy) it records and the stream, in 1080p30 files' worth.
/// (The software encoder has its own warning in the encoder check.)
fn size_verdict(jobs: &[(String, encode::VideoSettings)], proxies: bool, hardware: bool) -> Option<String> {
    if !hardware {
        return None;
    }
    let px = |s: &encode::VideoSettings| s.width as f64 * s.height as f64 * s.fps.max(1) as f64;
    let files = jobs.iter().filter(|(l, _)| l != "Stream").count();
    let mut load: f64 = jobs.iter().map(|(_, s)| px(s)).sum::<f64>() / HD30;
    if proxies {
        // Each file's proxy, at about a quarter of its pixels.
        load += jobs.iter().filter(|(l, _)| l != "Stream").map(|(_, s)| px(s) / 4.0).sum::<f64>() / HD30;
    }
    if load <= HARDWARE_LIMIT {
        return None;
    }
    let big = jobs.iter().filter(|(l, s)| l != "Stream" && s.width as u64 * s.height as u64 > 2560 * 1440).count();
    let at_4k = if big > 0 { format!(" ({big} of them 4K)") } else { String::new() };
    Some(format!(
        "This show records {files} files{at_4k}{}, about {:.0} times the work of one 1080p file. That may be too much for this computer: if the test recording repeats frames, record fewer sources (untick Record on some), use 1080p sources{}.",
        if jobs.iter().any(|(l, _)| l == "Stream") { " and streams" } else { "" },
        load,
        if proxies { ", or turn off proxy files" } else { "" }
    ))
}

/// Open every encoder in the plan at the same time and say whether they
/// all started, and on what.
pub fn check_encoders(plan: &EncoderPlan) -> CheckItem {
    const WHAT: &str = "Encoders";
    let mut open = Vec::new();
    let mut failed = Vec::new();
    let mut notes = Vec::new();
    let mut software = false;
    for (label, s) in &plan.jobs {
        match encode::start_video(s, std::sync::Arc::new(|_| {})) {
            Ok((e, backend, note)) => {
                software |= !encode::is_hardware(backend);
                if let Some(n) = note {
                    notes.push(format!("{label}: {n}"));
                }
                open.push(e);
            }
            Err(e) => failed.push((label.clone(), format!("{e:#}"))),
        }
    }
    for mut e in open {
        let _ = e.finish();
    }
    let n = plan.jobs.len();
    let big = plan.jobs.iter().any(|(_, s)| s.width as u64 * s.height as u64 > 1920 * 1080);
    verdict(n, &failed, &notes, software, big, &plan.description).map_or_else(|| item(Level::Ok, WHAT, ok_line(n, &plan.description)), |(l, d)| item(l, WHAT, d))
}

fn ok_line(n: usize, description: &str) -> String {
    if n == 1 {
        description.to_string()
    } else {
        format!("{description} · all {n} start together")
    }
}

/// What's wrong with the encoders, if anything: (level, sentence).
fn verdict(n: usize, failed: &[(String, String)], notes: &[String], software: bool, big: bool, description: &str) -> Option<(Level, String)> {
    if let Some((first, why)) = failed.first() {
        let names: Vec<&str> = failed.iter().map(|(l, _)| l.as_str()).collect();
        let text = if n == 1 {
            format!("The encoder won't start: {why}. Pick another encoder in Record › Recording.")
        } else {
            format!(
                "{} of {n} encoders couldn't start at the same time ({}; {first}: {why}). Those files would be empty. Record fewer sources (untick Record on some), use a smaller size, or pick another encoder in Record › Recording.",
                failed.len(),
                names.join(", ")
            )
        };
        return Some((Level::Fail, text));
    }
    if software && (n > 1 || big) {
        let mut text = "No graphics-card encoder is doing this (update the graphics driver). The software encoder can't keep up with more than one 1080p file, so expect dropped frames.".to_string();
        if !notes.is_empty() {
            text.push_str(&format!(" {}.", notes.join("; ")));
        }
        return Some((Level::Warn, text));
    }
    if !notes.is_empty() {
        return Some((Level::Warn, format!("{} · {}.", ok_line(n, description), notes.join("; "))));
    }
    None
}

/// The background half of the check: the encoders (when there's a plan)
/// and each stream destination, contacted without going live. Results
/// arrive on the channel as they come in.
pub fn check_in_background(plan: Option<EncoderPlan>, dests: Vec<StreamDestination>) -> Receiver<CheckItem> {
    let (tx, rx) = unbounded();
    if let Some(plan) = plan {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check_encoders(&plan));
        });
    }
    add_destinations(&tx, dests);
    rx
}

/// Contact each enabled stream destination without going live.
fn add_destinations(tx: &crossbeam_channel::Sender<CheckItem>, dests: Vec<StreamDestination>) {
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
                Err(e) => {
                    let raw = format!("{e:#}");
                    let (plain, _) = crate::engine::streamer::explain_failure(&raw, d.service);
                    item(Level::Fail, what, if plain == raw { raw } else { format!("{plain} ({raw})") })
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One encoder failing among several is a red line naming the files;
    /// the software encoder on a busy show is amber (review G3, G10).
    /// A big show gets a "may be too much" line; a usual one doesn't
    /// (review G9).
    #[test]
    fn show_size_line() {
        let file = |w: u32, h: u32| encode::VideoSettings { codec: crate::media::VideoCodec::H264, width: w, height: h, fps: 30, bitrate_kbps: 0, purpose: encode::Purpose::Record, backend: "auto".into() };
        let usual: Vec<_> = (0..5).map(|i| (format!("ISO {i}"), file(1920, 1080))).collect();
        assert_eq!(size_verdict(&usual, true, true), None);
        let heavy: Vec<_> = (0..5).map(|i| (format!("ISO {i}"), file(3840, 2160))).collect();
        let text = size_verdict(&heavy, false, true).unwrap();
        assert!(text.contains("5 files (5 of them 4K)") && text.contains("20 times"), "{text}");
        assert_eq!(size_verdict(&heavy, false, false), None, "the software encoder has its own warning");
    }

    #[test]
    fn encoder_verdicts() {
        assert_eq!(verdict(4, &[], &[], false, true, "H.264"), None);
        let failed = vec![("ISO 3 (Cam 3)".to_string(), "no more sessions".to_string())];
        let (level, text) = verdict(4, &failed, &[], false, false, "H.264").unwrap();
        assert_eq!(level, Level::Fail);
        assert!(text.contains("1 of 4") && text.contains("ISO 3 (Cam 3)"), "{text}");
        assert_eq!(verdict(1, &[], &[], true, false, "H.264"), None);
        assert_eq!(verdict(2, &[], &[], true, false, "H.264").unwrap().0, Level::Warn);
        assert_eq!(verdict(1, &[], &[], true, true, "H.264").unwrap().0, Level::Warn);
        let notes = vec!["ISO 2: NVIDIA couldn't start".to_string()];
        assert_eq!(verdict(3, &[], &notes, false, false, "H.264").unwrap().0, Level::Warn);
    }
}
