//! Starting and stopping a take: every file, its backup copy, timecode,
//! markers, and the side files (switch log, take notes, chapters, editor
//! timelines) written when it ends.

use super::export::{self, ExportFile, TakeExport};
use super::health::{self, DiskStatus, TakeMonitor};
use super::recorder::{self, FileOpts, Marker, Markers, VideoStats};
use super::{audio, encode, fmt_duration, next_boundary, podcast, switcher, Engine, LogLevel, ProgramTap, RecState, VIDEO_LATENCY};
use crate::config::TransitionMode;
use crate::media::mp4::Brand;
use crate::media::{clock, Ns, NS_PER_SEC};
use crate::util::{human_size, safe_file_name};
use anyhow::{bail, Context, Result};
use crossbeam_channel::{bounded, unbounded};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Local date/time and seconds since midnight at host time `t`.
fn wall_clock_at(t: Ns) -> (chrono::DateTime<chrono::Local>, f64) {
    use chrono::Timelike;
    let at = chrono::Local::now() + chrono::Duration::nanoseconds(t - clock::now());
    let secs = at.num_seconds_from_midnight() as f64 + at.nanosecond().min(999_999_999) as f64 / 1e9;
    (at, secs)
}

/// Why a take can't start on this drive, if it can't.
pub fn space_problem(d: &DiskStatus) -> Option<String> {
    let left = d.seconds_left();
    if d.free < health::MIN_START_BYTES || left.is_some_and(|s| s < health::MIN_START_SECONDS) {
        Some(format!(
            "not enough space on the drive for {} ({} free{}). Free up space or choose another folder in Settings → Recording",
            d.path.display(),
            human_size(d.free),
            left.map(|s| format!(", about {} of recording", health::fmt_left(s))).unwrap_or_default()
        ))
    } else {
        None
    }
}

impl Engine {
    pub fn is_recording(&self) -> bool {
        self.take.lock().unwrap().is_some()
    }

    /// Space on the recording drive at the current settings.
    pub fn disk_status(&self) -> Option<DiskStatus> {
        let cfg = self.config();
        let channels: Vec<(u32, bool)> = self.audio.iter().map(|a| (a.input.channels(), a.record)).collect();
        let rate = health::estimate_rate(&cfg, &channels);
        health::disk_space(&cfg.output.folder).map(|(free, total)| DiskStatus { path: cfg.output.folder.clone(), free, total, rate, measured: false })
    }

    pub fn start_recording(&self) -> Result<PathBuf> {
        let mut guard = self.take.lock().unwrap();
        if guard.is_some() {
            bail!("already recording");
        }
        if matches!(self.status.lock().unwrap().rec, RecState::Finalizing { .. }) {
            bail!("still finishing the previous take");
        }
        let cfg = self.config();
        let out = &cfg.output;
        std::fs::create_dir_all(&out.folder).with_context(|| format!("can't create the recording folder {}", out.folder.display()))?;
        if let Some(problem) = self.disk_status().as_ref().and_then(space_problem) {
            bail!("{problem}");
        }
        let codec = out.codec.video_codec();
        if let Some(p) = encode::codec_problem(codec) {
            bail!("{p}; choose H.264 or HEVC in Settings → Recording");
        }
        let backend = encode::resolve_backend(codec, &out.encoder)?;
        let aac = if self.audio.is_empty() { None } else { Some(encode::create_aac(audio::MIX_RATE, 2, out.audio_bitrate_kbps)?) };
        let audio_cfg = aac.as_ref().map(|a| a.config());
        // Each input on its own track in the Program file: (input, name, mono, encoder).
        let mut track_encoders = Vec::new();
        if out.input_tracks && aac.is_some() {
            for (i, a) in self.audio.iter().enumerate() {
                let mono = cfg.audio.iter().find(|c| c.name == a.name).is_some_and(|c| c.program_left == c.program_right);
                let ch: u16 = if mono { 1 } else { 2 };
                let enc = encode::create_aac(audio::MIX_RATE, ch, (out.audio_bitrate_kbps * ch as u32 / 2).max(64))?;
                track_encoders.push((i, a.name.clone(), mono, enc));
            }
        }
        let track_cfgs: Vec<(String, crate::media::AudioConfig)> = track_encoders.iter().map(|t| (t.1.clone(), t.3.config())).collect();

        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let prefix = safe_file_name(if out.take_prefix.trim().is_empty() { "Take" } else { &out.take_prefix });
        let mut name = format!("{prefix}_{stamp}");
        let mut n = 2;
        while out.folder.join(&name).exists() {
            name = format!("{prefix}_{stamp}_{n}");
            n += 1;
        }
        let folder = out.folder.join(&name);
        std::fs::create_dir_all(&folder).with_context(|| format!("can't create the output folder {}", folder.display()))?;

        // Backup copy on another drive: a problem there never stops the take.
        let backup_folder = match out.backup_folder.as_ref().filter(|p| !p.as_os_str().is_empty()) {
            Some(b) => {
                let dir = b.join(&name);
                let same_drive = std::fs::canonicalize(b).ok().zip(std::fs::canonicalize(&out.folder).ok()).is_some_and(|(a, b)| a == b);
                if same_drive {
                    self.status.lock().unwrap().push(LogLevel::Warning, "The backup folder is the recording folder itself, so no backup is written.");
                    None
                } else {
                    match std::fs::create_dir_all(&dir) {
                        Ok(()) => {
                            if let Some((free, _)) = health::disk_space(&dir) {
                                if free < health::MIN_START_BYTES {
                                    self.status.lock().unwrap().push(LogLevel::Warning, format!("The backup drive has only {} free; the backup will stop when it fills.", human_size(free)));
                                }
                            }
                            Some(dir)
                        }
                        Err(e) => {
                            self.status.lock().unwrap().push(LogLevel::Warning, format!("No backup for this take: can't create {} ({e}). Is the backup drive connected?", dir.display()));
                            None
                        }
                    }
                }
            }
            None => None,
        };

        let fps = self.info.fps;
        let t0 = next_boundary(fps);
        let t1 = Arc::new(AtomicI64::new(i64::MAX));
        let (wall, tc_secs) = wall_clock_at(t0);
        let markers: Markers = Arc::new(Mutex::new(Vec::new()));
        let opts = FileOpts {
            take_folder: folder.clone(),
            backup_folder: backup_folder.clone(),
            tc_seconds: out.timecode.then_some(tc_secs),
            tc_date: (wall.format("%Y-%m-%d").to_string(), wall.format("%H:%M:%S").to_string()),
            markers: markers.clone(),
        };
        let tc_for = |fps: u32| out.timecode.then(|| recorder::tc_frame(tc_secs, fps));
        let container = out.effective_container();
        let ext = container.extension();
        let brand = match container {
            crate::config::Container::Mov => Brand::Mov,
            crate::config::Container::Mp4 => Brand::Mp4,
        };
        let proxies = out.proxies;
        if proxies {
            let _ = std::fs::create_dir_all(folder.join("Proxies"));
            if let Some(b) = &backup_folder {
                let _ = std::fs::create_dir_all(b.join("Proxies"));
            }
        }
        let proxy_backend = if proxies { Some(encode::resolve_backend(crate::media::VideoCodec::H264, "auto")?) } else { None };
        let mut writers = Vec::new();
        let mut video_jobs = Vec::new();
        let mut audio_outs = Vec::new();
        let mut stats = Vec::new();
        let mut export_files = Vec::new();

        // One video file (and its proxy): returns where its frames go.
        let mut add_video = |label: String, file: String, fps: u32, size: (u32, u32), with_audio: bool, rate: u32, retimer: Option<recorder::Retimer>, queue: usize, tracks: Vec<(String, crate::media::AudioConfig)>, clips: Option<Arc<super::clips::ClipBuffer>>| {
            let path = folder.join(&file);
            let (mtx, mrx) = unbounded();
            let acfg = if with_audio { audio_cfg.clone() } else { None };
            writers.push((label.clone(), recorder::spawn_mp4(path.clone(), brand, fps, acfg.clone(), tracks.clone(), mrx, self.status.clone(), label.clone(), opts.clone(), clips)));
            let mut muxes = vec![mtx.clone()];
            if acfg.is_some() {
                audio_outs.push(mtx.clone());
            }
            let proxy = proxy_backend.map(|backend| {
                let plabel = format!("{label} proxy");
                let (ptx, prx) = unbounded();
                writers.push((plabel.clone(), recorder::spawn_mp4(folder.join("Proxies").join(&file), brand, fps, acfg.clone(), tracks.clone(), prx, self.status.clone(), plabel, opts.clone(), None)));
                muxes.push(ptx.clone());
                if acfg.is_some() {
                    audio_outs.push(ptx.clone());
                }
                let factor = recorder::proxy_factor(size.0, out.proxy_width);
                let pw = size.0 / factor;
                // Never more than the full-size file (ProRes rates are far higher anyway).
                let prate = encode::auto_bitrate_kbps(crate::media::VideoCodec::H264, pw, size.1 / factor, fps).clamp(1_500, 6_000).min(rate.max(300));
                recorder::ProxyJob { settings: recorder::video_settings(crate::media::VideoCodec::H264, fps, prate, backend), factor, mux: ptx }
            });
            let (vtx, vrx) = bounded(queue);
            let st = Arc::new(VideoStats::default());
            stats.push((label.clone(), st.clone()));
            video_jobs.push((
                label.clone(),
                recorder::spawn_video(label, vrx, recorder::video_settings(codec, fps, rate, backend), retimer, t0, t1.clone(), mtx, self.status.clone(), st, proxy),
            ));
            (path, vtx, muxes)
        };

        let program_muxes;
        // Program.
        {
            let (cw, ch) = self.info.canvas;
            let rate = encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, cw, ch, fps);
            // Kept for clips and replays, whichever needs more.
            let keep = out.clip_seconds.max(cfg.replay.seconds.ceil() as u32).clamp(5, 600);
            self.clips.start(folder.clone(), fps, brand, audio_cfg.clone(), keep);
            let (path, vtx, muxes) = add_video("Program".into(), format!("Program.{ext}"), fps, (cw, ch), true, rate, None, (fps as usize / 2).max(8), track_cfgs.clone(), Some(self.clips.clone()));
            export_files.push(ExportFile { path, name: "Program".into(), width: cw, height: ch, fps, has_audio: audio_cfg.is_some(), source: None, tc_frame: tc_for(fps) });
            self.video_taps.lock().unwrap().program = Some(ProgramTap { tx: vtx, from: t0 });
            program_muxes = muxes;
        }

        // The vertical version, with the recording mix.
        if cfg.vertical.enabled && cfg.vertical.record {
            let (vw, vh) = cfg.vertical.size();
            let rate = encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, vw, vh, fps);
            let (_, vtx, _) = add_video("Program (vertical)".into(), format!("Program_Vertical.{ext}"), fps, (vw, vh), true, rate, None, (fps as usize / 2).max(8), Vec::new(), None);
            self.video_taps.lock().unwrap().tall = Some(ProgramTap { tx: vtx, from: t0 });
        }

        // One file per source.
        for (i, s) in self.sources.iter().enumerate() {
            if !s.record {
                continue;
            }
            let label = format!("ISO {} ({})", i + 1, s.name);
            let file = format!("ISO_{:02}_{}.{ext}", i + 1, safe_file_name(&s.name));
            let with_audio = out.iso_audio && audio_cfg.is_some();
            let rate = encode::record_bitrate_kbps(codec, out.iso_bitrate_kbps, s.size.0, s.size.1, s.fps);
            // Short queue: these can be camera buffers, and holding many
            // starves the camera's buffer pool.
            let retimer = Some(recorder::Retimer::new(t0, s.fps, s.slot.delay));
            let (path, vtx, _) = add_video(label, file, s.fps, s.size, with_audio, rate, retimer, 8, Vec::new(), None);
            export_files.push(ExportFile { path, name: s.name.clone(), width: s.size.0, height: s.size.1, fps: s.fps, has_audio: with_audio, source: Some(i), tc_frame: tc_for(s.fps) });
            s.slot.start_iso_tap(vtx, t0);
        }

        // Recording mix, encoded once for every file.
        let mut mix_tap_ids = Vec::new();
        let mut tap = |kind: audio::MixKind| {
            let (atx, arx) = bounded(1000);
            let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
            self.mix_taps.lock().unwrap().push((id, kind, atx));
            mix_tap_ids.push(id);
            arx
        };
        let audio_job = aac.map(|aac| recorder::spawn_audio(tap(audio::MixKind::Recording), t0, t1.clone(), aac, audio_outs, None, false));
        // Each input's own track.
        let mut track_jobs = Vec::new();
        for (k, (i, name, mono, enc)) in track_encoders.into_iter().enumerate() {
            let rx = tap(audio::MixKind::Track { input: i, post_fader: out.input_tracks_post_fader });
            track_jobs.push((name, recorder::spawn_audio(rx, t0, t1.clone(), enc, program_muxes.clone(), Some(k), mono)));
        }
        drop(program_muxes);
        // The mix as a WAV (also the source of the podcast copy).
        let mut wavs = Vec::new();
        let mix_wav = (out.mix_wav || cfg.podcast.enabled) && !self.audio.is_empty();
        if mix_wav {
            let rx = tap(audio::MixKind::Recording);
            wavs.push(("Mix".to_string(), recorder::spawn_wav(rx, folder.join("Mix"), out.wav_bit_depth, false, t0, t1.clone(), self.status.clone(), opts.clone())));
        }

        // WAVs.
        for (i, a) in self.audio.iter().enumerate() {
            if !a.record {
                continue;
            }
            let base = folder.join(format!("Audio_{:02}_{}", i + 1, safe_file_name(&a.name)));
            let (wtx, wrx) = bounded(3000);
            wavs.push((a.name.clone(), recorder::spawn_wav(wrx, base, out.wav_bit_depth, a.split, t0, t1.clone(), self.status.clone(), opts.clone())));
            a.shared.start_raw_tap(wtx, t0);
        }

        let live = self.switcher.live();
        let take = recorder::Take {
            folder: folder.clone(),
            t0,
            t1,
            started: Instant::now(),
            first_scene: live.name.clone(),
            first_angle: live.full_frame_source(),
            video_jobs,
            audio_job,
            track_jobs,
            writers,
            wavs,
            mix_tap_ids,
            markers,
            opts,
            export_files,
            tc_seconds: tc_secs,
        };
        *self.take_monitor.lock().unwrap() = Some(TakeMonitor::new(folder.clone(), backup_folder.clone(), stats));
        {
            let mut st = self.status.lock().unwrap();
            st.rec = RecState::Recording { since: take.started, folder: folder.clone() };
            st.rec_failed = None;
            st.markers = 0;
            st.push(LogLevel::Info, format!("Recording to {}", folder.display()));
            if let Some(b) = &backup_folder {
                st.push(LogLevel::Info, format!("Backup copy to {}", b.display()));
            }
        }
        *guard = Some(take);
        self.mix.reset_recording_loudness();
        (self.waker)();
        Ok(folder)
    }

    /// Drop a marker at what the program shows now. Returns its name.
    pub fn add_marker(&self, name: Option<String>) -> Result<String> {
        let guard = self.take.lock().unwrap();
        let Some(take) = guard.as_ref() else { bail!("markers go into a recording; start recording first") };
        // The program monitor shows frames VIDEO_LATENCY behind real time.
        let at = (clock::now() - VIDEO_LATENCY - take.t0).max(0);
        let mut markers = take.markers.lock().unwrap();
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Marker {}", markers.len() + 1));
        markers.push(Marker { at, name: name.clone(), scene: self.switcher.live().name });
        markers.sort_by_key(|m| m.at);
        let count = markers.len();
        drop(markers);
        drop(guard);
        let mut st = self.status.lock().unwrap();
        st.markers = count;
        st.push(LogLevel::Info, format!("{name} at {}", fmt_duration(Duration::from_nanos(at as u64))));
        drop(st);
        (self.waker)();
        Ok(name)
    }

    /// Save the last `seconds` of the recording (the setting when None) as
    /// its own file in the take's Clips folder, marked in the take.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<String> {
        let number = self.clips.reserve()?;
        let seconds = seconds.unwrap_or(self.config().output.clip_seconds).clamp(1, 600);
        let name = format!("Clip {number}");
        let _ = self.add_marker(Some(name.clone()));
        let (clips, status, waker) = (self.clips.clone(), self.status.clone(), self.waker.clone());
        std::thread::Builder::new().name("save clip".into()).spawn(move || {
            // Let what's on screen now reach the file first.
            std::thread::sleep(Duration::from_nanos((VIDEO_LATENCY + 700_000_000) as u64));
            let msg = match clips.save(seconds, number) {
                Ok(clip) => {
                    let file = clip.path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    (LogLevel::Info, format!("Saved {file} ({:.0} s) in the take's Clips folder", clip.seconds))
                }
                Err(e) => (LogLevel::Warning, format!("{name} couldn't be saved: {e:#}")),
            };
            status.lock().unwrap().push(msg.0, msg.1);
            waker();
        })?;
        Ok(format!("Clip {number}"))
    }

    /// Stop recording. Files are finalized in the background; watch
    /// `status.rec` (or call `wait_finalized`).
    pub fn stop_recording(&self) {
        let Some(take) = self.take.lock().unwrap().take() else { return };
        self.clips.stop();
        let fps = self.info.fps;
        let t1 = next_boundary(fps);
        take.t1.store(t1, Ordering::Relaxed);
        let folder = take.folder.clone();
        self.status.lock().unwrap().rec = RecState::Finalizing { folder: folder.clone() };
        (self.waker)();

        let taps = self.video_taps.clone();
        let slots: Vec<(String, Arc<super::sources::SourceSlot>)> = self.sources.iter().map(|s| (s.name.clone(), s.slot.clone())).collect();
        let inputs: Vec<Arc<audio::InputShared>> = self.audio.iter().map(|a| a.shared.clone()).collect();
        let mix_taps = self.mix_taps.clone();
        let monitor = self.take_monitor.clone();
        let (status, switcher, waker) = (self.status.clone(), self.switcher.clone(), self.waker.clone());
        let summary = self.describe_setup();
        let cfg = self.config();
        let (cw, ch) = self.info.canvas;
        let stream_since = self.stream_started_at();
        let captions = self.captions().filter(|_| cfg.captions.enabled && cfg.captions.transcript);
        // Everything up to t1 has been captured, mixed and composited by then.
        let settle = (VIDEO_LATENCY + 2 * NS_PER_SEC / fps as i64).max(self.audio_latency + audio::BLOCK_NS).max(300_000_000) + 100_000_000;
        let handle = std::thread::spawn(move || {
            clock::sleep_until(t1 + settle);
            {
                let mut t = taps.lock().unwrap();
                t.program = None;
                t.tall = None;
            }
            let mut problems = Vec::new();
            for (name, s) in &slots {
                let dropped = s.stop_iso_tap();
                if dropped > 0 {
                    problems.push(format!("{name}: {dropped} frames were dropped because the encoder couldn't keep up (they were repeated in the file)"));
                }
            }
            mix_taps.lock().unwrap().retain(|(i, ..)| !take.mix_tap_ids.contains(i));
            for i in &inputs {
                i.stop_raw_tap();
            }
            let t0 = take.t0;
            let side = SideFiles {
                folder: folder.clone(),
                backup: take.opts.backup_folder.clone(),
                first_scene: take.first_scene.clone(),
                first_angle: take.first_angle,
                markers: take.markers.lock().unwrap().clone(),
                export_files: take.export_files.clone(),
                tc_seconds: take.tc_seconds,
            };
            let mut report = take.collect();
            *monitor.lock().unwrap() = None;
            // The last words of the take, once recognised.
            let cues = captions.map(|c| {
                c.settle(t1, Duration::from_secs(8));
                c.cues_between(t0, t1)
            });
            report.problems.extend(problems);
            report.duration = Duration::from_nanos((t1 - t0).max(0) as u64);
            let events = switcher.events_between(t0 + VIDEO_LATENCY, t1 + VIDEO_LATENCY);
            let written = side.write(&cfg, &report, &summary, &events, t0, t1, fps, (cw, ch), stream_since, cues.as_deref());
            let mut st = status.lock().unwrap();
            let total: u64 = report.files.iter().map(|f| f.1).sum();
            st.push(
                LogLevel::Info,
                format!("Saved {} files ({}), {} long, to {}", report.files.len(), human_size(total), fmt_duration(report.duration), folder.display()),
            );
            if !written.is_empty() {
                st.push(LogLevel::Info, format!("Also saved: {}", written.join(", ")));
            }
            for p in &report.problems {
                st.push(LogLevel::Warning, p.clone());
            }
            st.rec = RecState::Idle;
            st.last_take = Some(report);
            drop(st);
            waker();
            if cfg.podcast.enabled {
                make_podcast(&cfg, &folder, side.backup.as_deref(), &side.markers, &status, &waker);
            }
        });
        self.finalizers.lock().unwrap().push(handle);
    }
}

/// The podcast copy, made from the mix WAV after the take is saved.
fn make_podcast(cfg: &crate::config::Config, folder: &std::path::Path, backup: Option<&std::path::Path>, markers: &[Marker], status: &Arc<Mutex<super::Status>>, waker: &super::Waker) {
    let wav = folder.join("Mix.wav");
    if !wav.exists() {
        return;
    }
    let take_name = folder.file_name().unwrap_or_default().to_string_lossy().to_string();
    let file = format!("{take_name} (Podcast).m4a");
    let p = &cfg.podcast;
    let show = [p.show.trim(), cfg.ui.profile.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("").to_string();
    let now = chrono::Local::now();
    let title = if !p.title.trim().is_empty() {
        p.title.trim().to_string()
    } else if !show.is_empty() {
        format!("{show} – {}", now.format("%-d %B %Y"))
    } else {
        take_name.clone()
    };
    let episode = podcast::Episode { title, show, author: p.author.clone(), date: now.format("%Y-%m-%d").to_string() };
    let chapters: Vec<(Ns, String)> = markers.iter().map(|m| (m.at, m.name.clone())).collect();
    let backup_file = backup.map(|b| b.join(&file));
    status.lock().unwrap().background = Some(("Making the podcast".into(), 0.0));
    waker();
    let mut last = Instant::now();
    let result = podcast::make(&wav, &folder.join(&file), backup_file.as_deref(), p, &episode, &chapters, &mut |f| {
        if last.elapsed() > Duration::from_millis(250) {
            last = Instant::now();
            status.lock().unwrap().background = Some(("Making the podcast".into(), f));
            waker();
        }
    });
    let mut st = status.lock().unwrap();
    st.background = None;
    match result {
        Ok(r) => st.push(
            LogLevel::Info,
            format!("Podcast saved: {file} ({}, {}; the mix was {:.1} LUFS, levelled {:+.1} dB)", human_size(r.size), fmt_duration(Duration::from_secs_f64(r.seconds)), r.measured_lufs, r.gain_db),
        ),
        Err(e) => st.push(LogLevel::Warning, format!("The podcast copy couldn't be made: {e:#}")),
    }
    drop(st);
    waker();
}

/// What's needed to write a take's side files once its media is done.
struct SideFiles {
    folder: PathBuf,
    backup: Option<PathBuf>,
    first_scene: String,
    first_angle: Option<usize>,
    markers: Vec<Marker>,
    export_files: Vec<ExportFile>,
    tc_seconds: f64,
}

impl SideFiles {
    /// Writes the side files (and copies them to the backup). Returns the
    /// names of the optional ones written.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        cfg: &crate::config::Config,
        report: &recorder::TakeReport,
        setup: &str,
        events: &[switcher::SwitchEvent],
        t0: Ns,
        t1: Ns,
        fps: u32,
        canvas: (u32, u32),
        stream_since: Option<Ns>,
        cues: Option<&[super::captions::Cue]>,
    ) -> Vec<String> {
        let mut extra = Vec::new();
        let mut files: Vec<(String, String)> = Vec::new();
        files.push(("Switches.csv".into(), switch_log(t0, fps, &self.first_scene, events)));
        let take_name = self.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
        if let Some(cues) = cues.filter(|c| !c.is_empty()) {
            let (txt, srt, vtt) = super::captions::transcript_files(cues, t0, &take_name);
            files.push(("Transcript.txt".into(), txt));
            files.push(("Captions.srt".into(), srt));
            files.push(("Captions.vtt".into(), vtt));
            extra.push("transcript and caption files".to_string());
        }
        let marks: Vec<(Ns, String)> = self.markers.iter().map(|m| (m.at, m.name.clone())).collect();
        if !marks.is_empty() {
            files.push(("Chapters.txt".into(), export::youtube_chapters(&marks)));
            extra.push("Chapters.txt".to_string());
            // Chapters for the stream's own recording (it started at a different moment).
            if let Some(s) = stream_since {
                let shift = t0 - s;
                if shift.abs() > NS_PER_SEC {
                    let m: Vec<(Ns, String)> = marks.iter().map(|(t, n)| (t + shift, n.clone())).filter(|(t, _)| *t >= 0).collect();
                    files.push(("Chapters (stream).txt".into(), export::youtube_chapters(&m)));
                }
            }
        }
        // Editor timelines, only for files that were written.
        let written: HashSet<PathBuf> = report.files.iter().map(|f| f.0.clone()).collect();
        let mut timelines: Vec<(String, TakeExport)> = Vec::new();
        if cfg.output.export_timelines {
            let mut efiles: Vec<ExportFile> = self.export_files.iter().filter(|f| written.contains(&f.path)).cloned().collect();
            if efiles.iter().any(|f| f.source.is_none()) {
                // Video files only; timecode only when stamped.
                for f in &mut efiles {
                    if !cfg.output.timecode {
                        f.tc_frame = None;
                    }
                }
                let frames = ((t1 - t0).max(0) as i128 * fps as i128 / NS_PER_SEC as i128) as i64;
                let recorded: HashSet<usize> = efiles.iter().filter_map(|f| f.source).collect();
                let cuts = export::cuts_from_switches(&self.first_scene, self.first_angle, events, t0, VIDEO_LATENCY, fps, frames, &recorded);
                let markers = self.markers.iter().map(|m| ((m.at as i128 * fps as i128 / NS_PER_SEC as i128) as i64, m.name.clone())).collect();
                let te = TakeExport { name: take_name.clone(), fps, width: canvas.0, height: canvas.1, frames, files: efiles, cuts, markers };
                timelines.push((format!("{take_name}.fcpxml"), te.clone()));
                timelines.push((format!("{take_name} (Premiere).xml"), te));
                extra.push("Final Cut / Resolve and Premiere timelines".to_string());
            }
        }
        files.push(("Take.txt".into(), take_info(setup, report, &self.markers, self.tc_seconds, cfg, !timelines.is_empty())));

        // Projects for audio software and Resolve.
        let session = if cfg.output.daw_sessions || cfg.output.resolve_project {
            let marks: Vec<(f64, String)> = self.markers.iter().map(|m| (m.at as f64 / 1e9, m.name.clone())).collect();
            super::sessions::Session::from_folder(&self.folder, &marks, cfg.output.timecode.then_some(self.tc_seconds), fps).ok()
        } else {
            None
        };
        if let Some(s) = &session {
            if cfg.output.daw_sessions {
                files.push((format!("{take_name}.rpp"), s.reaper()));
                extra.push("REAPER and Studio Pro / Bitwig / Cubase sessions".to_string());
            }
            if cfg.output.resolve_project {
                extra.push("the DaVinci Resolve project script's take file".to_string());
            }
        }
        let videos: Vec<String> = self.export_files.iter().filter(|f| written.contains(&f.path)).filter_map(|f| f.path.file_name().map(|n| n.to_string_lossy().to_string())).collect();

        for dir in std::iter::once(&self.folder).chain(self.backup.iter()) {
            for (name, text) in &files {
                let _ = std::fs::write(dir.join(name), text);
            }
            for (name, te) in &timelines {
                // Point the timeline at the files next to it.
                let mut te = te.clone();
                for f in &mut te.files {
                    if let Some(n) = f.path.file_name() {
                        f.path = dir.join(n);
                    }
                }
                let text = if name.ends_with(".fcpxml") { export::fcpxml(&te) } else { export::premiere_xml(&te) };
                let _ = std::fs::write(dir.join(name), text);
            }
            if let Some(s) = &session {
                if cfg.output.daw_sessions {
                    let _ = s.write_dawproject(&dir.join(format!("{take_name}.dawproject")));
                }
                if cfg.output.resolve_project {
                    let timeline = timelines.iter().find(|t| t.0.ends_with(".fcpxml")).map(|t| t.0.as_str());
                    let _ = std::fs::write(dir.join(format!("{take_name} (Resolve).lua")), s.resolve_lua(dir, &videos, timeline));
                }
            }
        }
        // The Resolve script starts at the latest take.
        if let Some(dir) = crate::config::default_config_path().parent() {
            let _ = std::fs::write(super::sessions::last_take_file(dir), self.folder.to_string_lossy().as_bytes());
        }
        // The backup copy is only complete once every file arrived there.
        if let Some(b) = &self.backup {
            let missing: Vec<String> = report
                .files
                .iter()
                .filter_map(|(p, size)| {
                    let rel = p.strip_prefix(&self.folder).ok()?;
                    let ok = std::fs::metadata(b.join(rel)).is_ok_and(|m| m.len() == *size);
                    (!ok).then(|| rel.to_string_lossy().to_string())
                })
                .collect();
            if !missing.is_empty() {
                let _ = std::fs::write(b.join("BACKUP INCOMPLETE.txt"), format!("These files didn't fully reach the backup drive:\n{}\n\nThe main recording in {} is complete.\n", missing.join("\n"), self.folder.display()));
            } else {
                extra.push(format!("backup in {}", b.display()));
            }
        }
        extra
    }
}

pub(crate) fn timecode(ns: u64, fps: u32) -> String {
    let total_frames = ns as u128 * fps as u128 / 1_000_000_000;
    let fps = fps.max(1) as u128;
    let f = total_frames % fps;
    let s = total_frames / fps;
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f)
}

/// Edit decision list of every switch, relative to the start of the files.
/// Switches show up in the program `VIDEO_LATENCY` after the button press.
fn switch_log(t0: Ns, fps: u32, first_scene: &str, events: &[switcher::SwitchEvent]) -> String {
    let mut s = String::from("timecode,seconds,scene,transition,duration_ms\n");
    s += &format!("{},0.000,\"{}\",start,0\n", timecode(0, fps), first_scene.replace('"', "'"));
    for e in events {
        let ns = (e.at - VIDEO_LATENCY - t0).max(0) as u64;
        let mode = e.mode.name();
        let dur = if e.mode == TransitionMode::Cut { 0 } else { e.duration_ms };
        s += &format!("{},{:.3},\"{}\",{mode},{dur}\n", timecode(ns, fps), ns as f64 / 1e9, e.scene.replace('"', "'"));
    }
    s
}

fn take_info(setup: &str, report: &recorder::TakeReport, markers: &[Marker], tc_seconds: f64, cfg: &crate::config::Config, timelines: bool) -> String {
    let timecode_on = cfg.output.timecode;
    let mut s = format!("Cutaway take\nRecorded: {}\nLength: {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M"), fmt_duration(report.duration));
    if timecode_on {
        let t = tc_seconds as u64;
        s += &format!("Timecode at start: {:02}:{:02}:{:02} (time of day)\n", t / 3600, (t / 60) % 60, t % 60);
    }
    s += &format!("\n{setup}\nFiles:\n");
    for (f, size) in &report.files {
        let name = f.strip_prefix(&report.folder).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| f.file_name().unwrap_or_default().to_string_lossy().to_string());
        s += &format!("  {name}  ({})\n", human_size(*size));
    }
    s += "  Switches.csv  (every scene change, timed from the start of the files)\n";
    if cfg.output.input_tracks {
        let how = if cfg.output.input_tracks_post_fader { "as heard in the mix" } else { "before the fader" };
        s += &format!("  Program: track 1 is the mix; then each input on its own track ({how})\n");
    }
    if cfg.podcast.enabled {
        s += "  (Podcast).m4a  (made from Mix.wav after the take: levelled, with the markers as chapters)\n";
    }
    if cfg.captions.enabled && cfg.captions.transcript {
        s += "  Transcript.txt, Captions.srt, Captions.vtt  (from live captions; import the .srt into an editor or YouTube)\n";
    }
    if timelines {
        s += "  .fcpxml  (open in Final Cut Pro or DaVinci Resolve: a multicam clip and the live edit)\n";
        s += "  (Premiere).xml  (File → Import in Premiere Pro: the live edit, every camera on its own track)\n";
    }
    if !markers.is_empty() {
        s += "  Chapters.txt  (markers as a YouTube chapter list; paste into the video description)\n";
        s += "\nMarkers:\n";
        for m in markers {
            s += &format!("  {}  {}  (on air: {})\n", fmt_duration(Duration::from_nanos(m.at as u64)), m.name, m.scene);
        }
    }
    if !report.problems.is_empty() {
        s += "\nProblems:\n";
        for p in &report.problems {
            s += &format!("  {p}\n");
        }
    }
    s += "\nAll files start at the same instant, so they line up when placed at 00:00 on an editing timeline";
    s += if timecode_on { " (or sync them by timecode).\n" } else { ".\n" };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecodes() {
        assert_eq!(timecode(0, 30), "00:00:00:00");
        assert_eq!(timecode(1_500_000_000, 30), "00:00:01:15");
        assert_eq!(timecode(3_661_000_000_000, 25), "01:01:01:00");
    }

    #[test]
    fn refuses_a_nearly_full_drive() {
        let d = DiskStatus { path: "/x".into(), free: 50_000_000_000, total: 100_000_000_000, rate: 3_000_000.0, measured: false };
        assert!(space_problem(&d).is_none());
        let d = DiskStatus { free: 1_500_000_000, ..d };
        assert!(space_problem(&d).unwrap().contains("not enough space"));
        // Plenty of bytes but a huge data rate: under ten minutes.
        let d = DiskStatus { free: 3_000_000_000, rate: 10_000_000.0, ..d };
        assert!(space_problem(&d).is_some());
    }
}
