//! Starting and stopping a take: every file, its backup copy, timecode,
//! markers, and the side files (switch log, take notes, chapters, editor
//! timelines) written when it ends.

use crate::util::LockExt;
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
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
            "not enough space on the drive for {} ({}). Free up space or choose another folder in Record & stream › Quality and files",
            d.path.display(),
            d.describe(),
        ))
    } else {
        None
    }
}

impl Engine {
    pub fn is_recording(&self) -> bool {
        self.take.lock_ok().is_some()
    }

    /// Space on the recording drive at the current settings.
    pub fn disk_status(&self) -> Option<DiskStatus> {
        let cfg = self.config();
        let channels: Vec<(u32, bool)> = self.audio.iter().map(|a| (a.input.channels(), a.record)).collect();
        let estimate = health::estimate_rate(&cfg, &channels);
        // While recording, the settled measured rate (the header's figure), else the estimate.
        let live = self.status.lock_ok().disk.clone().filter(|d| d.measured);
        let (rate, measured) = live.map(|d| (d.rate, true)).unwrap_or((estimate, false));
        health::disk_space(&cfg.output.folder).map(|(free, total)| DiskStatus { path: cfg.output.folder.clone(), free, total, rate, measured })
    }

    pub fn start_recording(&self) -> Result<PathBuf> {
        let mut guard = self.take.lock_ok();
        if guard.is_some() {
            bail!("already recording");
        }
        if matches!(self.status.lock_ok().rec, RecState::Finalizing { .. }) {
            bail!("still finishing the previous take");
        }
        let cfg = self.config();
        let out = &cfg.output;
        std::fs::create_dir_all(&out.folder).with_context(|| format!("can't create the recording folder {}", out.folder.display()))?;
        if let Some(problem) = self.disk_status().as_ref().and_then(space_problem) {
            bail!("{problem}");
        }
        // Record & stream › Recordings lists takes from the folders used before, too.
        // (On a thread: a backup folder on a sleeping drive can be slow to look at.)
        if let Some(dir) = crate::config::default_config_path().parent().map(|d| d.to_path_buf()) {
            let output = out.clone();
            std::thread::spawn(move || crate::recovery::remember_folders(&dir, &crate::recovery::folders_of(&output)));
        }
        // A close-up recorded far bigger than the picture it holds: say so.
        for s in cfg.sources.iter().filter(|s| s.records_own_file(&cfg.sources)) {
            if let Some((size, detail)) = s.oversized(&cfg.sources) {
                self.status.lock_ok().push(LogLevel::Warning, crate::config::oversized_note(&s.name, size, detail));
            }
        }
        let codec = out.codec.video_codec();
        if let Some(p) = encode::codec_problem(codec) {
            bail!("{p}; choose H.264 or HEVC in Record & stream › Quality and files");
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
        let prefix = cfg.take_prefix();
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
                    self.status.lock_ok().push(LogLevel::Warning, "The backup folder is the recording folder itself, so no backup is written.");
                    None
                } else {
                    match std::fs::create_dir_all(&dir) {
                        Ok(()) => {
                            if let Some((free, _)) = health::disk_space(&dir) {
                                if free < health::MIN_START_BYTES {
                                    self.status.lock_ok().push(LogLevel::Warning, format!("The backup drive has only {} free; the backup will stop when it fills.", human_size(free)));
                                }
                            }
                            Some(dir)
                        }
                        Err(e) => {
                            self.status.lock_ok().push(LogLevel::Warning, format!("No backup for this recording: can't create {} ({e}). Is the backup drive connected?", dir.display()));
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
            let proxy = proxy_backend.map(|_| {
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
                recorder::ProxyJob { settings: recorder::video_settings(crate::media::VideoCodec::H264, fps, prate, "auto"), factor, mux: ptx }
            });
            let (vtx, vrx) = bounded(queue);
            let st = Arc::new(VideoStats::default());
            stats.push((label.clone(), st.clone()));
            video_jobs.push((
                label.clone(),
                recorder::spawn_video(label, vrx, recorder::video_settings(codec, fps, rate, if encode::is_auto(&out.encoder) { "auto" } else { backend }), retimer, t0, t1.clone(), mtx, self.status.clone(), st, proxy),
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
            export_files.push(ExportFile { path, name: "Program".into(), width: cw, height: ch, fps, has_audio: audio_cfg.is_some(), source: None, tc_frame: tc_for(fps), length: None, clean: false, start: 0, crop: None });
            self.video_taps.lock_ok().program = Some(ProgramTap { tx: vtx, from: t0 });
            program_muxes = muxes;
        }

        // The program without graphics, with the recording mix.
        if out.clean_program {
            let (cw, ch) = self.info.canvas;
            let rate = encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, cw, ch, fps);
            let (path, vtx, _) = add_video("Program (clean)".into(), format!("Program_Clean.{ext}"), fps, (cw, ch), true, rate, None, (fps as usize / 2).max(8), Vec::new(), None);
            export_files.push(ExportFile { path, name: "Program (clean)".into(), width: cw, height: ch, fps, has_audio: audio_cfg.is_some(), source: None, tc_frame: tc_for(fps), length: None, clean: true, start: 0, crop: None });
            self.video_taps.lock_ok().clean = Some(ProgramTap { tx: vtx, from: t0 });
        }

        // The vertical version, with the recording mix.
        if cfg.vertical.enabled && cfg.vertical.record {
            let (vw, vh) = cfg.vertical.size();
            let rate = encode::record_bitrate_kbps(codec, out.program_bitrate_kbps, vw, vh, fps);
            let (_, vtx, _) = add_video("Program (vertical)".into(), format!("Program_Vertical.{ext}"), fps, (vw, vh), true, rate, None, (fps as usize / 2).max(8), Vec::new(), None);
            self.video_taps.lock_ok().tall = Some(ProgramTap { tx: vtx, from: t0 });
        }

        // One file per source; the last stretch of each is kept for replays
        // from that camera (not ProRes: far too big to keep in memory).
        let keep_angles = !matches!(codec, crate::media::VideoCodec::ProRes(_));
        let mut angles = Vec::new();
        let mut iso_slots = Vec::new();
        let mut iso_numbers = Vec::new();
        for (i, s) in self.sources.iter().enumerate() {
            if !s.record {
                continue;
            }
            iso_slots.push((s.name.clone(), s.slot.clone()));
            let label = format!("ISO {} ({})", i + 1, s.name);
            // Files are numbered 01 to n with no gaps (sources not recorded
            // don't take a number); the name says which source it is.
            let file = format!("ISO_{:02}_{}.{ext}", iso_slots.len(), safe_file_name(&s.name));
            iso_numbers.push((s.uid, iso_slots.len()));
            let with_audio = out.iso_audio && audio_cfg.is_some();
            let rate = encode::record_bitrate_kbps(codec, out.iso_bitrate_kbps, s.size.0, s.size.1, s.fps);
            // Short queue: these can be camera buffers, and holding many
            // starves the camera's buffer pool.
            let retimer = Some(recorder::Retimer::new(t0, s.fps, s.slot.delay));
            let angle = keep_angles.then(|| {
                let b = super::clips::ClipBuffer::new();
                b.start(folder.clone(), s.fps, brand, None, cfg.replay.seconds.ceil() as u32);
                angles.push((s.name.clone(), b.clone(), None));
                b
            });
            let (path, vtx, _) = add_video(label, file, s.fps, s.size, with_audio, rate, retimer, 8, Vec::new(), angle);
            // (By identity: the switch log's angles stay right if sources move.)
            export_files.push(ExportFile { path, name: s.name.clone(), width: s.size.0, height: s.size.1, fps: s.fps, has_audio: with_audio, source: Some(s.uid as usize), tc_frame: tc_for(s.fps), length: None, clean: false, start: 0, crop: None });
            s.slot.start_iso_tap(vtx, t0);
        }
        // Close-ups saved as crops: no encoder, file or replay buffer of
        // their own. The timelines frame their shot from the wide camera's
        // file, and replays cut it from the wide camera's buffer.
        let mut crops = Vec::new();
        for s in self.sources.iter() {
            let Some((ef, virt)) = crop_file(s, &export_files, 0) else { continue };
            let wide = self.sources.iter().find(|w| Some(w.uid) == s.crop_of).map(|w| w.name.clone()).unwrap_or_default();
            if let Some(b) = angles.iter().find(|a| a.0 == wide).map(|a| a.1.clone()) {
                angles.push((s.name.clone(), b, Some(virt.clone())));
            }
            let first = ef.crop.as_ref().and_then(|c| c.keys.first()).map(|k| k.1).unwrap_or([0.0, 0.0, 1.0, 1.0]);
            export_files.push(ef);
            crops.push((s.uid, virt, Arc::new(Mutex::new(vec![(0, first)]))));
        }

        // Recording mix, encoded once for every file.
        let mut mix_tap_ids = Vec::new();
        let mut tap = |kind: audio::MixKind| {
            let (atx, arx) = bounded(1000);
            let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
            self.mix_taps.lock_ok().push((id, kind, atx));
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
        let mut raw_inputs = Vec::new();
        for a in self.audio.iter() {
            if !a.record {
                continue;
            }
            raw_inputs.push(a.shared.clone());
            let base = folder.join(format!("Audio_{:02}_{}", raw_inputs.len(), safe_file_name(&a.name)));
            let (wtx, wrx) = bounded(3000);
            wavs.push((a.name.clone(), recorder::spawn_wav(wrx, base, out.wav_bit_depth, a.split, t0, t1.clone(), self.status.clone(), opts.clone())));
            a.shared.start_raw_tap(wtx, t0);
        }

        let live = self.switcher.live();
        // The journal: what the side files need, kept as the take goes, for
        // a rescue after a crash.
        let journal = {
            let start = super::journal::Start {
                take: name.clone(),
                t0,
                fps,
                canvas: self.info.canvas,
                wall: wall.to_rfc3339(),
                tc_seconds: tc_secs,
                first_scene: live.name.clone(),
                first_angle: self.switcher.angle_now(),
                files: export_files.iter().map(journal_file).collect(),
                setup: self.describe_setup(),
                output: out.clone(),
            };
            let dirs: Vec<&std::path::Path> = std::iter::once(folder.as_path()).chain(backup_folder.as_deref()).collect();
            super::journal::Journal::create(&dirs, start).map(Arc::new)
        };
        self.switcher.set_journal(journal.clone());
        for (uid, virt, keys) in &crops {
            let first = keys.lock_ok()[0].1;
            virt.start_log(shot_log(*uid, t0, t0, first, keys.clone(), journal.clone()));
        }
        let take = recorder::Take {
            folder: folder.clone(),
            t0,
            t1,
            // The REC clock counts from the files' first frame (t0), as the
            // markers and the take's length do, not from when the encoders
            // finished opening.
            started: instant_at(t0),
            wall,
            first_scene: live.name.clone(),
            first_angle: self.switcher.angle_now(),
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
            iso_slots,
            raw_inputs,
            journal,
            late: recorder::LateFiles {
                brand,
                ext: ext.to_string(),
                codec,
                backend: backend.to_string(),
                proxy_backend: proxy_backend.map(|b| b.to_string()),
                proxy_width: out.proxy_width,
                iso_bitrate_kbps: out.iso_bitrate_kbps,
                tc_seconds: out.timecode.then_some(tc_secs),
            },
            iso_numbers,
            crops,
        };
        *self.take_monitor.lock_ok() = Some(TakeMonitor::new(folder.clone(), backup_folder.clone(), stats));
        {
            let mut st = self.status.lock_ok();
            st.rec = RecState::Recording { since: take.started, folder: folder.clone() };
            st.rec_failed = None;
            st.markers = 0;
            st.push(LogLevel::Info, format!("Recording to {}", folder.display()));
            if let Some(b) = &backup_folder {
                st.push(LogLevel::Info, format!("Backup copy to {}", b.display()));
            }
        }
        *self.angles.lock_ok() = angles;
        *guard = Some(take);
        self.mix.reset_recording_loudness();
        (self.waker)();
        Ok(folder)
    }

    /// Source `i` joined the show, or restarted, during the take: it gets
    /// a file of its own straight away, from the next program frame,
    /// named for when it starts ("ISO_03_Cam_2 (from 00-20-14).mp4") and
    /// placed there in the timelines. (No sound in it: the mix is in the
    /// program and Mix.wav.) Returns the file's name.
    pub(crate) fn start_late_iso(&self, i: usize) -> Option<String> {
        let s = self.sources.get(i)?;
        if s.crop_of.is_some() {
            // A close-up saved as a crop: no file, it carries on (or starts)
            // as a crop of its wide camera's file from here.
            let mut guard = self.take.lock_ok();
            let take = guard.as_mut()?;
            let from = next_boundary(self.info.fps) - take.t0;
            self.add_late_crop(take, s, from);
            return None;
        }
        if !s.record {
            return None;
        }
        let mut guard = self.take.lock_ok();
        let take = guard.as_mut()?;
        let late = take.late.clone();
        let fps = self.info.fps;
        // On the program's frame grid, like the take's start.
        let from = next_boundary(fps);
        let offset = from - take.t0;
        let number = match take.iso_numbers.iter().find(|n| n.0 == s.uid) {
            Some(n) => n.1,
            None => {
                let n = take.iso_numbers.iter().map(|n| n.1).max().unwrap_or(0) + 1;
                take.iso_numbers.push((s.uid, n));
                n
            }
        };
        let when = fmt_duration(Duration::from_nanos(offset.max(0) as u64)).replace(':', "-");
        let file = format!("ISO_{number:02}_{} (from {when}).{}", safe_file_name(&s.name), late.ext);
        let path = take.folder.join(&file);
        let label = format!("ISO {} ({}, from {when})", i + 1, s.name);
        let mut opts = take.opts.clone();
        opts.tc_seconds = late.tc_seconds.map(|t| t + offset as f64 / NS_PER_SEC as f64);
        let (mtx, mrx) = unbounded();
        take.writers.push((label.clone(), recorder::spawn_mp4(path.clone(), late.brand, s.fps, None, Vec::new(), mrx, self.status.clone(), label.clone(), opts.clone(), None)));
        let rate = encode::record_bitrate_kbps(late.codec, late.iso_bitrate_kbps, s.size.0, s.size.1, s.fps);
        let proxy = late.proxy_backend.as_ref().map(|backend| {
            let plabel = format!("{label} proxy");
            let (ptx, prx) = unbounded();
            take.writers.push((plabel.clone(), recorder::spawn_mp4(take.folder.join("Proxies").join(&file), late.brand, s.fps, None, Vec::new(), prx, self.status.clone(), plabel, opts.clone(), None)));
            let factor = recorder::proxy_factor(s.size.0, late.proxy_width);
            let prate = encode::auto_bitrate_kbps(crate::media::VideoCodec::H264, s.size.0 / factor, s.size.1 / factor, s.fps).clamp(1_500, 6_000).min(rate.max(300));
            recorder::ProxyJob { settings: recorder::video_settings(crate::media::VideoCodec::H264, s.fps, prate, backend), factor, mux: ptx }
        });
        let (vtx, vrx) = bounded(8);
        let st = Arc::new(VideoStats::default());
        let retimer = Some(recorder::Retimer::new(from, s.fps, s.slot.delay));
        take.video_jobs.push((label.clone(), recorder::spawn_video(label.clone(), vrx, recorder::video_settings(late.codec, s.fps, rate, &late.backend), retimer, from, take.t1.clone(), mtx, self.status.clone(), st.clone(), proxy)));
        let ef = ExportFile {
            path,
            name: s.name.clone(),
            width: s.size.0,
            height: s.size.1,
            fps: s.fps,
            has_audio: false,
            source: Some(s.uid as usize),
            tc_frame: opts.tc_seconds.map(|t| recorder::tc_frame(t, s.fps)),
            length: None,
            clean: false,
            start: offset,
            crop: None,
        };
        if let Some(j) = &take.journal {
            j.add(&super::journal::Entry::File(journal_file(&ef)));
        }
        take.export_files.push(ef);
        take.iso_slots.push((s.name.clone(), s.slot.clone()));
        s.slot.start_iso_tap(vtx, from);
        if let Some(m) = self.take_monitor.lock_ok().as_mut() {
            m.add_file(label, st);
        }
        // Its close-ups saved as crops follow it into its new file.
        for c in self.sources.iter().filter(|c| c.crop_of == Some(s.uid)) {
            self.add_late_crop(take, c, offset);
        }
        Some(file)
    }

    /// Close-up `s`, saved as a crop, from `from` (from the start of the
    /// take): in the wide camera's newest file, its shot noted from now.
    fn add_late_crop(&self, take: &mut recorder::Take, s: &super::SourceHandle, from: Ns) {
        let Some((ef, virt)) = crop_file(s, &take.export_files, from) else { return };
        // Once for each file (the close-up and its wide camera may both
        // have restarted).
        if take.export_files.iter().any(|f| f.crop.is_some() && f.source == ef.source && f.path == ef.path) {
            return;
        }
        let first = ef.crop.as_ref().and_then(|c| c.keys.first()).map(|k| k.1).unwrap_or([0.0, 0.0, 1.0, 1.0]);
        if let Some(j) = &take.journal {
            j.add(&super::journal::Entry::File(journal_file(&ef)));
        }
        take.export_files.push(ef);
        let keys = match take.crops.iter_mut().find(|c| c.0 == s.uid) {
            Some(c) => {
                // (Remade: its controls are new.)
                c.1.stop_log();
                c.1 = virt.clone();
                c.2.clone()
            }
            None => {
                let keys = Arc::new(Mutex::new(Vec::new()));
                take.crops.push((s.uid, virt.clone(), keys.clone()));
                keys
            }
        };
        keys.lock_ok().push((from, first));
        virt.start_log(shot_log(s.uid, take.t0, take.t0 + from, first, keys, take.journal.clone()));
    }

    /// Drop a marker at what the program shows now. Returns its name.
    pub fn add_marker(&self, name: Option<String>) -> Result<String> {
        self.add_marker_as(name, false).map(|m| m.name)
    }

    /// The same, returning the marker (its time identifies it for
    /// `rename_marker` and `delete_marker`).
    pub fn add_marker_now(&self, name: Option<String>) -> Result<Marker> {
        self.add_marker_as(name, false)
    }

    /// This take's markers (and clips), in time order; none when not recording.
    pub fn take_markers(&self) -> Vec<Marker> {
        self.take.lock_ok().as_ref().map(|t| t.markers.lock_ok().clone()).unwrap_or_default()
    }

    /// Rename the marker at `at` in the take being recorded (they're
    /// written when it stops).
    pub fn rename_marker(&self, at: Ns, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a marker needs a name");
        }
        self.edit_markers(|m| rename_marker_in(m, at, name), super::journal::Entry::Rename { at, name: name.to_string() })
    }

    /// Take the marker at `at` out of the take being recorded.
    pub fn delete_marker(&self, at: Ns) -> Result<()> {
        self.edit_markers(|m| remove_marker_in(m, at), super::journal::Entry::Delete { at })
    }

    fn edit_markers(&self, f: impl FnOnce(&mut Vec<Marker>) -> bool, logged: super::journal::Entry) -> Result<()> {
        let guard = self.take.lock_ok();
        let Some(take) = guard.as_ref() else { bail!("the recording has stopped, so its markers are saved") };
        let mut markers = take.markers.lock_ok();
        if !f(&mut markers) {
            bail!("that marker isn't in this recording any more");
        }
        if let Some(j) = &take.journal {
            j.add(&logged);
        }
        let count = markers.len();
        drop(markers);
        drop(guard);
        self.status.lock_ok().markers = count;
        (self.waker)();
        Ok(())
    }

    fn add_marker_as(&self, name: Option<String>, clip: bool) -> Result<Marker> {
        let guard = self.take.lock_ok();
        let Some(take) = guard.as_ref() else { bail!("markers go into a recording; start recording first") };
        // The program monitor shows frames VIDEO_LATENCY behind real time.
        let at = (clock::now() - VIDEO_LATENCY - take.t0).max(0);
        let mut markers = take.markers.lock_ok();
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Marker {}", markers.len() + 1));
        let marker = Marker { at, name: name.clone(), scene: self.switcher.live().name, clip };
        markers.push(marker.clone());
        markers.sort_by_key(|m| m.at);
        if let Some(j) = &take.journal {
            j.add(&super::journal::Entry::Marker(marker.clone()));
        }
        let count = markers.len();
        drop(markers);
        drop(guard);
        let mut st = self.status.lock_ok();
        st.markers = count;
        st.push(LogLevel::Info, format!("{name} at {}", fmt_duration(Duration::from_nanos(at as u64))));
        drop(st);
        (self.waker)();
        Ok(marker)
    }

    /// Save the last `seconds` of the recording (the setting when None) as
    /// its own file in the take's Clips folder, marked in the take.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<String> {
        let number = self.clips.reserve()?;
        let seconds = seconds.unwrap_or(self.config().output.clip_seconds).clamp(1, 600);
        let name = format!("Clip {number}");
        let _ = self.add_marker_as(Some(name.clone()), true);
        let (clips, status, waker) = (self.clips.clone(), self.status.clone(), self.waker.clone());
        std::thread::Builder::new().name("save clip".into()).spawn(move || {
            // Let what's on screen now reach the file first.
            std::thread::sleep(Duration::from_nanos((VIDEO_LATENCY + 700_000_000) as u64));
            let msg = match clips.save(seconds, number) {
                Ok(clip) => {
                    let file = clip.path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    (LogLevel::Info, format!("Saved {file} ({:.0} s) in the recording's Clips folder", clip.seconds))
                }
                Err(e) => (LogLevel::Warning, format!("{name} couldn't be saved: {e:#}")),
            };
            status.lock_ok().push(msg.0, msg.1);
            waker();
        })?;
        Ok(format!("Clip {number}"))
    }

    /// Stop recording. Files are finalized in the background; watch
    /// `status.rec` (or call `wait_finalized`).
    pub fn stop_recording(&self) {
        // (Finalizing is set before the take is let go of, so a new one
        // can't start in between and lose its program to this one's ending.)
        let mut guard = self.take.lock_ok();
        let Some(take) = guard.take() else { return };
        self.switcher.set_journal(None);
        for (_, virt, _) in &take.crops {
            virt.stop_log();
        }
        let folder = take.folder.clone();
        self.status.lock_ok().rec = RecState::Finalizing { folder: folder.clone(), step: "Catching the last frames".into(), done: 0.0 };
        drop(guard);
        self.clips.stop();
        for (_, a, _) in self.angles.lock_ok().drain(..) {
            a.stop();
        }
        let fps = self.info.fps;
        let t1 = next_boundary(fps);
        take.t1.store(t1, Ordering::Relaxed);
        (self.waker)();

        let taps = self.video_taps.clone();
        // The take's own: sources may have been added or removed since it started.
        let slots = take.iso_slots.clone();
        let inputs = take.raw_inputs.clone();
        let mix_taps = self.mix_taps.clone();
        let monitor = self.take_monitor.clone();
        let (status, switcher, waker) = (self.status.clone(), self.switcher.clone(), self.waker.clone());
        let graphics = self.graphics.clone();
        let summary = self.describe_setup();
        let cfg = self.config();
        let (cw, ch) = self.info.canvas;
        let stream_since = self.stream_started_at();
        let captions = self.captions().filter(|_| cfg.captions.enabled && cfg.captions.transcript);
        // Everything up to t1 has been captured, mixed and composited by then.
        let settle = (VIDEO_LATENCY + 2 * NS_PER_SEC / fps as i64).max(self.audio_latency + audio::BLOCK_NS).max(300_000_000) + 100_000_000;
        let (status2, waker2, folder2) = (status.clone(), waker.clone(), folder.clone());
        let podcast_stop = self.podcast_stop.clone();
        let handle = std::thread::spawn(move || {
            // A bug while saving mustn't leave Cutaway "saving" for good (no
            // new take could start). The files stay, and so does the journal,
            // so the next start can rebuild the notes.
            let saved = crate::util::guard("saving the recording", move || {
                clock::sleep_until(t1 + settle);
                {
                    let mut t = taps.lock_ok();
                    t.program = None;
                    t.tall = None;
                    t.clean = None;
                }
                let mut problems = Vec::new();
                for (name, s) in &slots {
                    let dropped = s.stop_iso_tap();
                    if dropped > 0 {
                        problems.push(format!("{name}: {dropped} frames were dropped because the encoder couldn't keep up (they were repeated in the file)"));
                    }
                }
                mix_taps.lock_ok().retain(|(i, ..)| !take.mix_tap_ids.contains(i));
                for i in &inputs {
                    i.stop_raw_tap();
                }
                let t0 = take.t0;
                let wall = take.wall;
                let side = SideFiles {
                    folder: folder.clone(),
                    backup: take.opts.backup_folder.clone(),
                    first_scene: take.first_scene.clone(),
                    first_angle: take.first_angle,
                    markers: take.markers.lock_ok().clone(),
                    export_files: with_shots(&take.export_files, &take.crops),
                    tc_seconds: take.tc_seconds,
                    on_air: graphics.on_air(take.t0, t1),
                    rescued: None,
                    note_latest: true,
                };
                let journal = take.journal.clone();
                // What the window shows while it saves.
                let saving = |what: String, done: f32| {
                    if let RecState::Finalizing { step, done: d, .. } = &mut status.lock_ok().rec {
                        *step = what;
                        *d = done;
                    }
                    waker();
                };
                // Most of the work: the encoders finish and the files close
                // (90% of the bar); then the notes and timelines.
                let mut report = take.collect_with(|what, f| saving(what, f * 0.9));
                *monitor.lock_ok() = None;
                // The last words of the take, once recognised.
                let cues = captions.map(|c| {
                    saving("Waiting for the last captions".into(), 0.9);
                    c.settle(t1, Duration::from_secs(8));
                    c.cues_between(t0, t1)
                });
                report.problems.extend(problems);
                report.duration = Duration::from_nanos((t1 - t0).max(0) as u64);
                report.markers = side.markers.iter().filter(|m| !m.clip).map(|m| (m.at as f64 / NS_PER_SEC as f64, m.name.clone())).collect();
                report.podcast_pending = cfg.podcast.enabled && folder.join("Mix.wav").exists();
                let events = switcher.events_between(t0 + VIDEO_LATENCY, t1 + VIDEO_LATENCY);
                saving("Writing the timeline and notes".into(), 0.95);
                let (written, backup_problem) = side.write(&cfg, &report, &summary, &events, t0, t1, wall, fps, (cw, ch), stream_since, cues.as_deref());
                report.problems.extend(backup_problem);
                // Saved: the journal kept for a rescue isn't needed.
                if let Some(j) = journal {
                    j.remove();
                }
                let mut st = status.lock_ok();
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
                let pending = report.podcast_pending;
                st.last_take = Some(report);
                drop(st);
                waker();
                if pending {
                    let made = make_podcast(&cfg, &folder, side.backup.as_deref(), &side.markers, wall, &status, &waker, &podcast_stop);
                    // Now uploads can go, with the podcast.
                    if let Some(r) = status.lock_ok().last_take.as_mut().filter(|r| r.folder == folder) {
                        r.podcast_pending = false;
                        r.podcast = made;
                    }
                    waker();
                }
                Ok(())
            });
            if let Err(e) = saved {
                let mut st = status2.lock_ok();
                if matches!(st.rec, RecState::Finalizing { .. }) {
                    st.rec = RecState::Idle;
                }
                if st.background.as_ref().is_some_and(|b| b.0 == "Making the podcast") {
                    st.background = None;
                }
                if let Some(r) = st.last_take.as_mut().filter(|r| r.folder == folder2) {
                    r.podcast_pending = false;
                }
                st.push(LogLevel::Error, format!("{e:#}. The recording's files are in {}", folder2.display()));
                drop(st);
                waker2();
            }
        });
        self.finalizers.lock_ok().push(handle);
    }
}

/// The podcast copy, made from the mix WAV after the take is saved.
/// Returns the file, when made.
#[allow(clippy::too_many_arguments)]
fn make_podcast(cfg: &crate::config::Config, folder: &std::path::Path, backup: Option<&std::path::Path>, markers: &[Marker], recorded: chrono::DateTime<chrono::Local>, status: &Arc<Mutex<super::Status>>, waker: &super::Waker, stop: &AtomicBool) -> Option<PathBuf> {
    let wav = folder.join("Mix.wav");
    if !wav.exists() {
        return None;
    }
    let take_name = folder.file_name().unwrap_or_default().to_string_lossy().to_string();
    let file = format!("{take_name} (Podcast).m4a");
    let p = &cfg.podcast;
    let show = [p.show.trim(), cfg.ui.profile.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("").to_string();
    // The day the show was recorded, not when it finished saving.
    let now = recorded;
    let title = if !p.title.trim().is_empty() {
        p.title.trim().to_string()
    } else if !show.is_empty() {
        format!("{show} – {}", now.format("%-d %B %Y"))
    } else {
        take_name.clone()
    };
    let episode = podcast::Episode { title, show, author: p.author.clone(), date: now.format("%Y-%m-%d").to_string() };
    let chapters: Vec<(Ns, String)> = markers.iter().filter(|m| !m.clip).map(|m| (m.at, m.name.clone())).collect();
    let backup_file = backup.map(|b| b.join(&file));
    status.lock_ok().background = Some(("Making the podcast".into(), 0.0));
    waker();
    let mut last = Instant::now();
    let result = podcast::make(&wav, &folder.join(&file), backup_file.as_deref(), p, &episode, &chapters, &mut |f| {
        if last.elapsed() > Duration::from_millis(250) {
            last = Instant::now();
            status.lock_ok().background = Some(("Making the podcast".into(), f));
            waker();
        }
    }, stop);
    let mut st = status.lock_ok();
    st.background = None;
    let made = match result {
        Ok(r) => {
            st.push(
                LogLevel::Info,
                format!("Podcast saved: {file} ({}, {}; the mix was {:.1} LUFS, levelled {:+.1} dB)", human_size(r.size), fmt_duration(Duration::from_secs_f64(r.seconds)), r.measured_lufs, r.gain_db),
            );
            Some(folder.join(&file))
        }
        Err(_) if stop.load(Ordering::Relaxed) => {
            st.push(LogLevel::Info, format!("The podcast copy of {take_name} wasn't finished: Cutaway quit (Mix.wav is kept)"));
            None
        }
        Err(e) => {
            st.push(LogLevel::Warning, format!("The podcast copy couldn't be made: {e:#}"));
            None
        }
    };
    drop(st);
    waker();
    made
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
    /// Graphics on screen during the take (the sponsor report).
    on_air: Vec<super::graphics::OnAir>,
    /// Rescued after a crash: what the repair did to each file.
    rescued: Option<Vec<String>>,
    /// Note this as the latest take (for the Resolve script).
    note_latest: bool,
}

impl SideFiles {
    /// Writes the side files (and copies them to the backup). Returns the
    /// names of the optional ones written, and what went wrong with the backup.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        cfg: &crate::config::Config,
        report: &recorder::TakeReport,
        setup: &str,
        events: &[switcher::SwitchEvent],
        t0: Ns,
        t1: Ns,
        recorded: chrono::DateTime<chrono::Local>,
        fps: u32,
        canvas: (u32, u32),
        stream_since: Option<Ns>,
        cues: Option<&[super::captions::Cue]>,
    ) -> (Vec<String>, Option<String>) {
        let mut extra = Vec::new();
        let mut backup_problem = None;
        let mut files: Vec<(String, String)> = Vec::new();
        files.push(("Switches.csv".into(), switch_log(t0, fps, &self.first_scene, events)));
        if !self.on_air.is_empty() {
            files.push(("Graphics on air.csv".into(), graphics_log(t0, fps, &self.on_air)));
            extra.push("Graphics on air.csv".to_string());
        }
        let take_name = self.folder.file_name().unwrap_or_default().to_string_lossy().to_string();
        if let Some(cues) = cues.filter(|c| !c.is_empty()) {
            let (txt, srt, vtt) = super::captions::transcript_files(cues, t0, &take_name);
            files.push(("Transcript.txt".into(), txt));
            files.push(("Captions.srt".into(), srt));
            files.push(("Captions.vtt".into(), vtt));
            extra.push("transcript and caption files".to_string());
        }
        // Chapters: the markers (not the clips), by YouTube's rules.
        let secs = |ns: Ns| ns as f64 / NS_PER_SEC as f64;
        let marks: Vec<(f64, String)> = self.markers.iter().filter(|m| !m.clip).map(|m| (secs(m.at), m.name.clone())).collect();
        let chapters = export::youtube_chapters(&marks, secs(t1 - t0));
        if let Some(c) = &chapters {
            files.push(("Chapters.txt".into(), format!("{c}\n")));
            extra.push("Chapters.txt".to_string());
            // Chapters for the stream's own recording (it started at a different moment).
            if let Some(s) = stream_since {
                let shift = secs(t0 - s);
                if shift.abs() > 1.0 {
                    let m: Vec<(f64, String)> = marks.iter().map(|(t, n)| (t + shift, n.clone())).filter(|(t, _)| *t >= 0.0).collect();
                    if let Some(c) = export::youtube_chapters(&m, secs(t1 - s)) {
                        files.push(("Chapters (stream).txt".into(), format!("{c}\n")));
                    }
                }
            }
        }
        // Editor timelines, only for files that were written.
        let written: HashSet<PathBuf> = report.files.iter().map(|f| f.0.clone()).collect();
        let mut timelines: Vec<(String, TakeExport)> = Vec::new();
        // The live edit: for the editor timelines, and the cut list the
        // Resolve script lays down (the same shots).
        let mut live_edit: Option<TakeExport> = None;
        if cfg.output.export_timelines || cfg.output.resolve_project {
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
                if cfg.output.export_timelines {
                    timelines.push((format!("{take_name}.fcpxml"), te.clone()));
                    timelines.push((format!("{take_name} (Premiere).xml"), te.clone()));
                    extra.push("Final Cut / Resolve and Premiere timelines".to_string());
                }
                live_edit = Some(te);
            }
        }
        files.push(("Take.txt".into(), take_info(setup, report, &self.markers, self.tc_seconds, recorded, cfg, !timelines.is_empty(), chapters.is_some(), self.rescued.as_deref())));

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

        let have_mix = session.as_ref().is_some_and(|s| s.tracks.iter().any(|t| t.file == "Mix.wav"));
        let resolve_edit = live_edit.as_ref().map(|te| super::sessions::ResolveEdit::from_export(te, have_mix)).unwrap_or_default();

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
                    let _ = std::fs::write(dir.join(format!("{take_name} (Resolve).lua")), s.resolve_lua(dir, &videos, timeline, canvas, &resolve_edit));
                }
            }
        }
        // The Resolve script starts at the latest take.
        if let Some(dir) = crate::config::default_config_path().parent().filter(|_| self.note_latest) {
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
                backup_problem = Some(format!("The backup copy didn't finish: {} of the files didn't fully reach {} (see BACKUP INCOMPLETE.txt there)", missing.len(), b.display()));
                let _ = std::fs::write(b.join("BACKUP INCOMPLETE.txt"), format!("These files didn't fully reach the backup drive:\n{}\n\nThe main recording in {} is complete.\n", missing.join("\n"), self.folder.display()));
            } else {
                extra.push(format!("backup in {}", b.display()));
            }
        }
        (extra, backup_problem)
    }
}

/// The entry for close-up `s`, saved as a crop, in the timelines: the
/// wide camera's file (the newest, still going) from `start` (from the
/// start of the take), with the shot it has now. None when it isn't a
/// crop, or there's no such file.
fn crop_file(s: &super::SourceHandle, files: &[ExportFile], start: Ns) -> Option<(ExportFile, Arc<super::ptz::VirtualPtz>)> {
    let wide = s.crop_of?;
    let virt = s.input.ptz().and_then(|p| p.virt.clone())?;
    let file = files.iter().rev().find(|f| f.source == Some(wide as usize) && f.crop.is_none() && f.length.is_none())?;
    let first = super::ptz::shot_fraction(virt.position(), (file.width, file.height), s.size);
    let crop = export::Crop { of: wide as usize, size: s.size, keys: vec![(start, first)] };
    Some((ExportFile { name: s.name.clone(), has_audio: false, source: Some(s.uid as usize), length: None, clean: false, start: start.max(file.start), crop: Some(crop), ..file.clone() }, virt))
}

/// Notes a crop's shot as it moves: in the take (for Stop) and its journal
/// (for a rescue).
/// `since`: when (host time) `first` was noted.
fn shot_log(uid: u64, t0: Ns, since: Ns, first: [f32; 4], keys: Arc<Mutex<Vec<(Ns, [f32; 4])>>>, journal: Option<Arc<super::journal::Journal>>) -> super::ptz::ShotLog {
    super::ptz::ShotLog::new(
        since,
        first,
        Box::new(move |at, rect| {
            let at = (at - t0).max(0);
            keys.lock_ok().push((at, rect));
            if let Some(j) = &journal {
                j.add(&super::journal::Entry::Shot { source: uid as usize, at, rect });
            }
        }),
    )
}

/// The take's entries with every crop's shots filled in.
fn with_shots(files: &[ExportFile], crops: &[recorder::CropLog]) -> Vec<ExportFile> {
    let mut files = files.to_vec();
    for f in &mut files {
        let Some(c) = f.crop.as_mut() else { continue };
        if let Some((_, _, keys)) = crops.iter().find(|k| Some(k.0 as usize) == f.source) {
            let mut all = keys.lock_ok().clone();
            all.sort_by_key(|k| k.0);
            // Its first shot where its file starts, then every move after.
            let first = c.keys.first().copied();
            c.keys = all;
            if let Some(first) = first {
                if c.keys.first().is_none_or(|k| k.0 > first.0) {
                    c.keys.insert(0, first);
                }
            }
        }
    }
    files
}

/// A file as the journal keeps it: by its name in the take folder (the
/// take may be rescued from its backup copy, or after it's moved).
pub(crate) fn journal_file(f: &ExportFile) -> ExportFile {
    let mut f = f.clone();
    if let Some(n) = f.path.file_name() {
        f.path = PathBuf::from(n);
    }
    f
}

/// A rescued take's side files from its journal: the same Switches.csv,
/// timelines, Resolve take file, chapters and markers a normal Stop
/// writes, as far as the files go. `files` are the media files that were
/// repaired (and their sizes), `length` how long the take is now,
/// `repair` what was done to each file. Returns what was written.
pub(crate) fn rebuild_from_journal(dir: &std::path::Path, r: &super::journal::Replay, files: Vec<(PathBuf, u64)>, length: Duration, repair: Vec<String>) -> Vec<String> {
    let s = &r.start;
    let mut cfg = crate::config::Config::default();
    cfg.output = s.output.clone();
    // Made after Stop, so not for a rescued take.
    cfg.podcast.enabled = false;
    cfg.captions.transcript = false;
    let fps = s.fps.max(1);
    let t0 = s.t0;
    let t1 = t0 + length.as_nanos().min(i64::MAX as u128) as Ns;
    let wall = chrono::DateTime::parse_from_rfc3339(&s.wall).map(|w| w.with_timezone(&chrono::Local)).unwrap_or_else(|_| chrono::Local::now());
    let export_files = r.files.iter().map(|f| ExportFile { path: dir.join(&f.path), ..f.clone() }).collect();
    let report = recorder::TakeReport { folder: dir.to_path_buf(), files, duration: length, recorded_at: Some(wall), ..Default::default() };
    // The same switches Stop takes (those showing in the program by the end).
    let events: Vec<switcher::SwitchEvent> = r.events.iter().filter(|e| e.at >= t0 + VIDEO_LATENCY && e.at <= t1 + VIDEO_LATENCY).cloned().collect();
    let side = SideFiles {
        folder: dir.to_path_buf(),
        backup: None,
        first_scene: s.first_scene.clone(),
        first_angle: s.first_angle,
        markers: r.markers.clone(),
        export_files,
        tc_seconds: s.tc_seconds,
        on_air: Vec::new(),
        rescued: Some(repair),
        note_latest: false,
    };
    side.write(&cfg, &report, &s.setup, &events, t0, t1, wall, fps, s.canvas, None, None).0
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
/// A name for a quoted CSV field: one line, no double quotes, and never
/// read as a formula when opened in a spreadsheet (a scene called
/// "=HYPERLINK(…)" stays text: a ' goes in front, as spreadsheets expect).
fn csv_text(name: &str) -> String {
    let t = crate::util::one_line(name).replace('"', "'");
    if t.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{t}")
    } else {
        t
    }
}

fn switch_log(t0: Ns, fps: u32, first_scene: &str, events: &[switcher::SwitchEvent]) -> String {
    let mut s = String::from("timecode,seconds,scene,transition,duration_ms\n");
    s += &format!("{},0.000,\"{}\",start,0\n", timecode(0, fps), csv_text(first_scene));
    for e in events {
        let ns = (e.at - VIDEO_LATENCY - t0).max(0) as u64;
        let mode = e.mode.name();
        let dur = if e.mode == TransitionMode::Cut { 0 } else { e.duration_ms };
        s += &format!("{},{:.3},\"{}\",{mode},{dur}\n", timecode(ns, fps), ns as f64 / 1e9, csv_text(&e.scene));
    }
    s
}

/// Each graphic shown in the take: how often, for how long, and when
/// (timed from the start of the files), for sponsors and the edit.
fn graphics_log(t0: Ns, fps: u32, on_air: &[super::graphics::OnAir]) -> String {
    let mut s = String::from("graphic,times_shown,seconds_on_screen,on_screen_at\n");
    for o in on_air {
        let at: Vec<String> = o.showings.iter().map(|(a, b)| format!("{} - {}", timecode((a - t0).max(0) as u64, fps), timecode((b - t0).max(0) as u64, fps))).collect();
        s += &format!("\"{}\",{},{:.1},\"{}\"\n", csv_text(&o.name), o.showings.len(), o.seconds(), at.join("; "));
    }
    s
}

#[allow(clippy::too_many_arguments)]
/// The `Instant` of a moment on the media clock (now or earlier; a moment
/// still ahead is fine too).
fn instant_at(t: Ns) -> Instant {
    let (now, now_ns) = (Instant::now(), clock::now());
    let ago = now_ns - t;
    if ago >= 0 {
        now.checked_sub(Duration::from_nanos(ago as u64)).unwrap_or(now)
    } else {
        now + Duration::from_nanos(ago.unsigned_abs())
    }
}

#[allow(clippy::too_many_arguments)]
fn take_info(setup: &str, report: &recorder::TakeReport, markers: &[Marker], tc_seconds: f64, recorded: chrono::DateTime<chrono::Local>, cfg: &crate::config::Config, timelines: bool, chapters: bool, rescued: Option<&[String]>) -> String {
    let timecode_on = cfg.output.timecode;
    let head = if rescued.is_some() { "Cutaway take (recovered)" } else { "Cutaway take" };
    let mut s = format!("{head}\nRecorded: {}\nLength: {}\n", recorded.format("%Y-%m-%d %H:%M"), fmt_duration(report.duration));
    if rescued.is_some() {
        s += "\nThis take wasn't stopped normally (Cutaway closed, or the computer stopped, while recording).\n\
              Its files were repaired and play up to about the last second that was written. The switches\n\
              and markers were saved as the recording went, so its timelines, chapters and markers are here too.\n";
    }
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
        s += "  (Podcast).m4a  (made from Mix.wav after the recording: levelled, with the markers as chapters)\n";
    }
    if cfg.captions.enabled && cfg.captions.transcript {
        s += "  Transcript.txt, Captions.srt, Captions.vtt  (from live captions; import the .srt into an editor or YouTube)\n";
    }
    if timelines {
        s += "  .fcpxml  (open in Final Cut Pro or DaVinci Resolve: a multicam clip and the live edit)\n";
        s += "  (Premiere).xml  (File → Import in Premiere Pro: the live edit, every camera on its own track)\n";
    }
    if chapters {
        s += "  Chapters.txt  (markers as a YouTube chapter list; paste into the video description)\n";
    }
    if !markers.is_empty() {
        s += "\nMarkers:\n";
        for m in markers {
            s += &format!("  {}  {}  (on air: {})\n", fmt_duration(Duration::from_nanos(m.at as u64)), m.name, m.scene);
        }
    }
    if let Some(lines) = rescued.filter(|l| !l.is_empty()) {
        s += "\nRepair:\n";
        for l in lines {
            s += &format!("{l}\n");
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

/// Rename the marker at `at`; false when there's none.
fn rename_marker_in(markers: &mut [Marker], at: Ns, name: &str) -> bool {
    markers.iter_mut().find(|m| m.at == at).map(|m| m.name = name.to_string()).is_some()
}

/// Take out the marker at `at`; false when there's none.
fn remove_marker_in(markers: &mut Vec<Marker>, at: Ns) -> bool {
    let before = markers.len();
    markers.retain(|m| m.at != at);
    markers.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_names_are_never_formulas() {
        assert_eq!(csv_text("=HYPERLINK(\"http://x\")"), "'=HYPERLINK('http://x')");
        assert_eq!(csv_text("+1"), "'+1");
        assert_eq!(csv_text("@sum"), "'@sum");
        assert_eq!(csv_text("Two shot\nnext"), "Two shot next");
        assert_eq!(csv_text("Wide"), "Wide");
    }

    #[test]
    fn timecodes() {
        assert_eq!(timecode(0, 30), "00:00:00:00");
        assert_eq!(timecode(1_500_000_000, 30), "00:00:01:15");
        assert_eq!(timecode(3_661_000_000_000, 25), "01:01:01:00");
    }

    /// The REC clock's zero is the files' first frame: a take whose first
    /// frame was 2 s ago reads 2 s, whenever the encoders finished opening.
    #[test]
    fn rec_clock_counts_from_the_first_frame() {
        let ago = instant_at(clock::now() - 2 * NS_PER_SEC).elapsed().as_secs_f64();
        assert!((ago - 2.0).abs() < 0.05, "{ago}");
        let ahead = instant_at(clock::now() + NS_PER_SEC / 2);
        assert_eq!(ahead.elapsed(), Duration::ZERO, "not started yet: 0, not negative");
    }

    #[test]
    fn notes_say_when_the_take_started() {
        use chrono::TimeZone;
        let at = chrono::Local.with_ymd_and_hms(2026, 10, 20, 23, 50, 0).single().unwrap();
        let report = recorder::TakeReport { duration: Duration::from_secs(1800), ..Default::default() };
        let cfg = crate::config::Config::default();
        let clip = Marker { at: 0, name: "Clip 1".into(), scene: "Wide".into(), clip: true };
        let s = take_info("", &report, &[clip], 0.0, at, &cfg, false, false, None);
        assert!(s.contains("Recorded: 2026-10-20 23:50"), "{s}");
        assert!(!s.contains("Chapters.txt") && s.contains("Clip 1"), "the clip is listed, but there are no chapters");
    }

    /// A take rescued from its journal gets the same side files a normal
    /// Stop writes: switch log, chapters, markers, timelines, Resolve file.
    #[test]
    fn a_journal_rebuilds_what_stop_writes() {
        use super::super::journal::{self, Entry, Journal};
        let root = std::env::temp_dir().join(format!("cutaway-journal-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let name = "Show_2026-11-01_20-04-12";
        let (a, b) = (root.join("stopped").join(name), root.join("rescued").join(name));
        let mut cfg = crate::config::Config::default();
        cfg.output.export_timelines = true;
        cfg.output.resolve_project = true;
        cfg.output.timecode = true;
        cfg.podcast.enabled = false;
        let t0: Ns = 1_000 * NS_PER_SEC;
        let length = Duration::from_secs(120);
        let t1 = t0 + 120 * NS_PER_SEC;
        let wall = chrono::DateTime::parse_from_rfc3339("2026-11-01T20:04:12+01:00").unwrap().with_timezone(&chrono::Local);
        let file = |dir: &std::path::Path, f: &str, source| ExportFile { path: dir.join(f), name: f.trim_end_matches(".mp4").into(), width: 1920, height: 1080, fps: 30, has_audio: true, source, tc_frame: Some(72_252 * 30), length: None, clean: false, start: 0, crop: None };
        let files_of = |dir: &std::path::Path| vec![file(dir, "Program.mp4", None), file(dir, "ISO_01_Wide.mp4", Some(7)), file(dir, "ISO_02_Host.mp4", Some(9))];
        let sw = |s: i64, scene: &str, angle| switcher::SwitchEvent { at: t0 + VIDEO_LATENCY + s * NS_PER_SEC, scene: scene.into(), mode: TransitionMode::Cut, duration_ms: 0, angle };
        let events = vec![sw(20, "Host", Some(9)), sw(45, "Both", None), sw(80, "Wide", Some(7))];
        let mk = |s: i64, n: &str| Marker { at: s * NS_PER_SEC, name: n.into(), scene: "Wide".into(), clip: false };
        let markers = vec![mk(0, "Welcome"), mk(30, "News"), mk(70, "Guest")];
        let mut cut = files_of(&a);
        cut[2].length = Some(100 * NS_PER_SEC); // Host left the show
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
            for f in files_of(d) {
                std::fs::write(&f.path, b"x").unwrap();
            }
        }
        let report_of = |dir: &std::path::Path| recorder::TakeReport { folder: dir.to_path_buf(), files: files_of(dir).iter().map(|f| (f.path.clone(), 1)).collect(), duration: length, recorded_at: Some(wall), ..Default::default() };
        // A normal Stop.
        let side = SideFiles { folder: a.clone(), backup: None, first_scene: "Wide".into(), first_angle: Some(7), markers: markers.clone(), export_files: cut, tc_seconds: 72_252.0, on_air: Vec::new(), rescued: None, note_latest: false };
        side.write(&cfg, &report_of(&a), "Setup", &events, t0, t1, wall, 30, (1920, 1080), None, None);
        // The same take, from its journal (markers renamed and taken out on the way).
        let start = journal::Start {
            take: name.into(),
            t0,
            fps: 30,
            canvas: (1920, 1080),
            wall: wall.to_rfc3339(),
            tc_seconds: 72_252.0,
            first_scene: "Wide".into(),
            first_angle: Some(7),
            files: files_of(&b).iter().map(journal_file).collect(),
            setup: "Setup".into(),
            output: cfg.output.clone(),
        };
        let j = Journal::create(&[b.as_path()], start).unwrap();
        j.add(&Entry::Marker(mk(0, "Welcome")));
        j.add(&Entry::Switch(events[0].clone()));
        j.add(&Entry::Marker(mk(30, "Marker 2")));
        j.add(&Entry::Marker(mk(50, "Oops")));
        j.add(&Entry::Delete { at: 50 * NS_PER_SEC });
        j.add(&Entry::Switch(events[1].clone()));
        j.add(&Entry::Rename { at: 30 * NS_PER_SEC, name: "News".into() });
        j.add(&Entry::Marker(mk(70, "Guest")));
        j.add(&Entry::Switch(events[2].clone()));
        j.add(&Entry::Ends { source: 9, length: 100 * NS_PER_SEC });
        j.close();
        let replay = journal::read(&b).unwrap();
        rebuild_from_journal(&b, &replay, report_of(&b).files, length, vec!["  Program.mp4: recovered 120.0 s".into()]);

        let read = |d: &std::path::Path, f: &str| std::fs::read_to_string(d.join(f)).unwrap_or_else(|_| panic!("{f} missing in {}", d.display()));
        for f in ["Switches.csv", "Chapters.txt"] {
            assert_eq!(read(&a, f), read(&b, f), "{f}");
        }
        assert!(read(&a, "Switches.csv").contains("\"Host\",cut"));
        let same_place = |t: String| t.replace("rescued", "stopped");
        for f in [format!("{name}.fcpxml"), format!("{name} (Premiere).xml"), format!("{name} (Resolve).lua")] {
            assert_eq!(read(&a, &f), same_place(read(&b, &f)), "{f}");
        }
        assert!(read(&b, &format!("{name} (Resolve).lua")).contains("\n  edit = {"));
        let (stopped, rescued) = (read(&a, "Take.txt"), read(&b, "Take.txt"));
        let markers_of = |t: &str| t[t.find("\nMarkers:").unwrap()..].split("\n\n").next().unwrap().to_string();
        assert_eq!(markers_of(&stopped), markers_of(&rescued));
        assert!(rescued.starts_with("Cutaway take (recovered)\nRecorded: ") && rescued.contains("Length: ") && rescued.contains("Repair:\n  Program.mp4: recovered"), "{rescued}");
        assert!(rescued.contains("  ISO_01_Wide.mp4  ("), "the files are listed with their sizes: {rescued}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn markers_are_renamed_and_removed_by_time() {
        let m = |at, name: &str| Marker { at, name: name.into(), scene: "Wide".into(), clip: false };
        let mut v = vec![m(10, "Marker 1"), m(20, "Marker 2")];
        assert!(rename_marker_in(&mut v, 20, "Guest arrives"));
        assert_eq!(v[1].name, "Guest arrives");
        assert!(!rename_marker_in(&mut v, 30, "Nope"));
        assert!(remove_marker_in(&mut v, 10));
        assert_eq!(v.len(), 1);
        assert!(!remove_marker_in(&mut v, 10));
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
