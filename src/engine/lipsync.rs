//! Automatic lip sync: clap once where every camera sees it and the mics
//! hear it; Cutaway finds the clap in each picture and each input and
//! works out the delays that line them all up.
//!
//! In a picture the clap is the moment fast motion stops (hands meeting);
//! in sound it's the sharp onset of the loudest transient. Times are on
//! the program clock, so current delays are taken into account.

use super::audio::MixKind;
use super::Engine;
use crate::media::{AudioBlock, Ns, VideoFrame, NS_PER_SEC};
use crossbeam_channel::{bounded, Receiver};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GRID_W: usize = 32;
const GRID_H: usize = 18;

/// A small grey picture (average luma per cell) of a frame.
pub fn luma_grid(f: &VideoFrame) -> Vec<u8> {
    let (w, h) = (f.width as usize, f.height as usize);
    let mut out = vec![0u8; GRID_W * GRID_H];
    if w < GRID_W || h < GRID_H {
        return out;
    }
    f.pixels.with_planes(|p| {
        for gy in 0..GRID_H {
            for gx in 0..GRID_W {
                let (x0, x1) = (gx * w / GRID_W, (gx + 1) * w / GRID_W);
                let (y0, y1) = (gy * h / GRID_H, (gy + 1) * h / GRID_H);
                // Every other pixel is plenty.
                let (mut sum, mut n) = (0u32, 0u32);
                for y in (y0..y1).step_by(2) {
                    for x in (x0..x1).step_by(2) {
                        sum += p.y[y * p.y_stride + x] as u32;
                        n += 1;
                    }
                }
                out[gy * GRID_W + gx] = (sum / n.max(1)) as u8;
            }
        }
    });
    out
}

/// When the clap happened in a run of pictures: (time, confidence). Looks
/// within [from, to] (times as given).
pub fn find_clap_video(frames: &[(Ns, Vec<u8>)], from: Ns, to: Ns) -> Option<(Ns, f32)> {
    if frames.len() < 4 {
        return None;
    }
    // Motion into each frame.
    let motion: Vec<(Ns, f32)> = frames
        .windows(2)
        .map(|w| {
            let d: u32 = w[0].1.iter().zip(&w[1].1).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs()).sum();
            (w[1].0, d as f32 / w[1].1.len().max(1) as f32)
        })
        .collect();
    let (peak_i, &(_, peak)) = motion.iter().enumerate().filter(|(_, m)| m.0 >= from && m.0 <= to).max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))?;
    let mut sorted: Vec<f32> = motion.iter().map(|m| m.1).collect();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2].max(0.05);
    let confidence = peak / median;
    // The hands meet in the last frame of the burst of motion.
    let threshold = peak * 0.35;
    let mut i = peak_i;
    while i + 1 < motion.len() && motion[i + 1].1 >= threshold {
        i += 1;
    }
    Some((motion[i].0, confidence))
}

/// When the clap sounded: (time of the onset, confidence). `samples` are
/// mono at 48 kHz starting at `start`.
pub fn find_clap_audio(samples: &[f32], start: Ns, from: Ns, to: Ns) -> Option<(Ns, f32)> {
    let at = |i: usize| start + i as i64 * NS_PER_SEC / 48_000;
    let index = |t: Ns| (((t - start).max(0) as i128 * 48_000 / NS_PER_SEC as i128) as usize).min(samples.len());
    let (lo, hi) = (index(from), index(to));
    if hi <= lo + 480 {
        return None;
    }
    // Envelope over 1 ms steps.
    let env: Vec<f32> = samples.chunks(48).map(|c| c.iter().fold(0.0f32, |m, s| m.max(s.abs()))).collect();
    let (elo, ehi) = (lo / 48, hi.div_ceil(48).min(env.len()));
    let (peak_ms, &peak) = env[elo..ehi].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, v)| (i + elo, v))?;
    if peak < 0.01 {
        return None;
    }
    let mut sorted: Vec<f32> = env[elo..ehi].to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2].max(1e-4);
    // Walk back to where it started rising (within 60 ms).
    let threshold = peak * 0.3;
    let mut onset = peak_ms;
    while onset > elo && peak_ms - onset < 60 && env[onset - 1] >= threshold {
        onset -= 1;
    }
    // Then to the exact sample.
    let first = (onset * 48..(peak_ms + 1) * 48).find(|&i| samples.get(i).is_some_and(|s| s.abs() >= threshold)).unwrap_or(onset * 48);
    Some((at(first), peak / median))
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyncItem {
    pub name: String,
    /// The clap's time (program clock), if found.
    pub event: Option<Ns>,
    pub confidence: f32,
    pub current_ms: u32,
    pub suggested_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncResult {
    pub video: Vec<SyncItem>,
    pub audio: Vec<SyncItem>,
    pub problem: Option<String>,
}

/// New delays from where each clap landed: the latest one sets the time,
/// everything else waits for it (and nothing waits needlessly).
pub fn suggest(result: &mut SyncResult) {
    let raws: Vec<Ns> = result.video.iter().chain(&result.audio).filter_map(|i| i.event.map(|e| e - i.current_ms as i64 * 1_000_000)).collect();
    let Some(&latest) = raws.iter().max() else { return };
    for i in result.video.iter_mut().chain(result.audio.iter_mut()) {
        i.suggested_ms = match i.event {
            Some(e) => ((latest - (e - i.current_ms as i64 * 1_000_000)) as f64 / 1e6).round().clamp(0.0, 2000.0) as u32,
            None => i.current_ms,
        };
    }
}

/// A measurement in progress.
pub struct SyncJob {
    /// 0..1 while listening.
    progress: AtomicU32,
    pub result: Mutex<Option<SyncResult>>,
}

impl SyncJob {
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

impl Engine {
    /// Listen to every source and input for `seconds` and find the clap.
    pub fn measure_sync(&self, seconds: f32) -> Arc<SyncJob> {
        let job = Arc::new(SyncJob { progress: AtomicU32::new(0), result: Mutex::new(None) });
        let cfg = self.config();
        // Taps on every source and input.
        let mut videos: Vec<(String, u32, u64, Arc<super::sources::SourceSlot>, Receiver<VideoFrame>)> = Vec::new();
        for (i, s) in self.sources.iter().enumerate() {
            // Auto-framed sources share their camera's timing.
            if cfg.sources.get(i).is_some_and(|c| c.framed_from().is_some()) {
                continue;
            }
            let (tx, rx) = bounded(240);
            let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
            s.slot.add_watcher(id, tx);
            let delay = cfg.sources.get(i).map(|c| c.delay_ms).unwrap_or(0);
            videos.push((s.name.clone(), delay, id, s.slot.clone(), rx));
        }
        let mut audios: Vec<(String, u32, u64, Receiver<AudioBlock>)> = Vec::new();
        for (i, a) in self.audio.iter().enumerate() {
            let (tx, rx) = bounded(2000);
            let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
            self.mix_taps.lock().unwrap().push((id, MixKind::Track { input: i, post_fader: false }, tx));
            let delay = cfg.audio.get(i).map(|c| c.delay_ms).unwrap_or(0);
            audios.push((a.name.clone(), delay, id, rx));
        }
        let (taps, waker, j2) = (self.mix_taps.clone(), self.waker.clone(), job.clone());
        std::thread::Builder::new()
            .name("lip sync".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs_f32(seconds.max(2.0));
                let mut frames: Vec<Vec<(Ns, Vec<u8>)>> = vec![Vec::new(); videos.len()];
                let mut sound: Vec<(Option<Ns>, Vec<f32>)> = vec![(None, Vec::new()); audios.len()];
                while Instant::now() < deadline {
                    for (k, v) in videos.iter().enumerate() {
                        while let Ok(f) = v.4.try_recv() {
                            // Program clock: when the picture is shown.
                            frames[k].push((f.pts + v.1 as i64 * 1_000_000, luma_grid(&f)));
                        }
                    }
                    for (k, a) in audios.iter().enumerate() {
                        while let Ok(b) = a.3.try_recv() {
                            let s = &mut sound[k];
                            s.0.get_or_insert(b.pts);
                            let ch = b.channels.max(1) as usize;
                            s.1.extend(b.samples.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
                        }
                    }
                    let elapsed = 1.0 - deadline.saturating_duration_since(Instant::now()).as_secs_f32() / seconds.max(2.0);
                    j2.progress.store(elapsed.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                    waker();
                    std::thread::sleep(Duration::from_millis(20));
                }
                for v in &videos {
                    v.3.remove_watcher(v.2);
                }
                taps.lock().unwrap().retain(|t| !audios.iter().any(|a| a.2 == t.0));
                let result = analyse(&videos.iter().map(|v| (v.0.clone(), v.1)).collect::<Vec<_>>(), &frames, &audios.iter().map(|a| (a.0.clone(), a.1)).collect::<Vec<_>>(), &sound);
                *j2.result.lock().unwrap() = Some(result);
                j2.progress.store(1.0f32.to_bits(), Ordering::Relaxed);
                waker();
            })
            .expect("thread");
        job
    }
}

/// Find the clap everywhere and suggest delays.
pub fn analyse(videos: &[(String, u32)], frames: &[Vec<(Ns, Vec<u8>)>], audios: &[(String, u32)], sound: &[(Option<Ns>, Vec<f32>)]) -> SyncResult {
    let mut result = SyncResult::default();
    // The loudest, clearest clap in the sound sets the moment to look around.
    let mut audio_found: Vec<Option<(Ns, f32)>> = sound.iter().map(|(start, s)| start.and_then(|st| find_clap_audio(s, st, Ns::MIN / 2, Ns::MAX / 2))).collect();
    let reference = audio_found.iter().flatten().max_by(|a, b| a.1.total_cmp(&b.1)).map(|a| a.0);
    let Some(reference) = reference.filter(|_| audio_found.iter().flatten().any(|a| a.1 > 3.0)) else {
        result.problem = Some("no clap was heard: clap once, sharply, close enough to a mic".into());
        for (name, d) in videos {
            result.video.push(SyncItem { name: name.clone(), event: None, confidence: 0.0, current_ms: *d, suggested_ms: *d });
        }
        for (name, d) in audios {
            result.audio.push(SyncItem { name: name.clone(), event: None, confidence: 0.0, current_ms: *d, suggested_ms: *d });
        }
        return result;
    };
    let window = 600_000_000;
    // Other inputs: the same clap, near the reference.
    for (k, (start, s)) in sound.iter().enumerate() {
        audio_found[k] = start.and_then(|st| find_clap_audio(s, st, reference - window, reference + window)).filter(|a| a.1 > 3.0);
    }
    for (k, (name, d)) in audios.iter().enumerate() {
        let f = audio_found[k];
        result.audio.push(SyncItem { name: name.clone(), event: f.map(|f| f.0), confidence: f.map(|f| f.1).unwrap_or(0.0), current_ms: *d, suggested_ms: *d });
    }
    for (k, (name, d)) in videos.iter().enumerate() {
        let f = find_clap_video(&frames[k], reference - window, reference + window).filter(|v| v.1 > 4.0);
        result.video.push(SyncItem { name: name.clone(), event: f.map(|f| f.0), confidence: f.map(|f| f.1).unwrap_or(0.0), current_ms: *d, suggested_ms: *d });
    }
    if result.video.iter().all(|v| v.event.is_none()) {
        result.problem = Some("no camera saw the clap: clap where the cameras can see your hands".into());
    }
    suggest(&mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testsrc::TestPattern;

    #[test]
    fn finds_claps_and_suggests_delays() {
        // Two cameras: one shows the clap on time, one 100 ms late; the
        // sound arrives on time.
        let fps = 30;
        let mut frames = vec![Vec::new(), Vec::new()];
        for (k, lag) in [(0usize, 0i64), (1, 100_000_000)] {
            let mut tp = TestPattern::new("clap", 128, 72);
            for n in 0..(3 * fps) {
                let t = 4 * NS_PER_SEC + n as i64 * NS_PER_SEC / fps as i64;
                let img = tp.render(t - lag, fps as u32);
                let f = VideoFrame::new_nv12(128, 72, t, img.y, img.uv, 0);
                frames[k].push((t, luma_grid(&f)));
            }
        }
        // Background tone with one clap at 5 s.
        let start = 4 * NS_PER_SEC;
        let mut s: Vec<f32> = (0..48_000 * 3).map(|i| 0.1 * (i as f32 * 0.0576).sin()).collect();
        for i in 0..2400 {
            s[48_000 + i] += 0.9 * (-(i as f32) / 400.0).exp() * if i % 7 < 3 { 1.0 } else { -1.0 };
        }
        // A second input hears it 40 ms later (and quieter).
        let mut late = vec![0.0f32; 1920];
        late.extend(s.iter().map(|v| v * 0.5));
        late.truncate(s.len());
        let (a, c) = find_clap_audio(&s, start, 0, i64::MAX).unwrap();
        assert!((a - 5 * NS_PER_SEC).abs() < 1_000_000, "{a}");
        assert!(c > 3.0);
        let (v, _) = find_clap_video(&frames[0], 4_500_000_000, 5_500_000_000).unwrap();
        assert_eq!(v, 5 * NS_PER_SEC);
        let (v2, _) = find_clap_video(&frames[1], 4_500_000_000, 5_500_000_000).unwrap();
        assert!((v2 - (5 * NS_PER_SEC + 100_000_000)).abs() <= NS_PER_SEC / 30, "{v2}");
        let r = analyse(
            &[("On time".into(), 0), ("Late".into(), 0)],
            &frames,
            &[("Mic".into(), 0), ("Far mic".into(), 20)],
            &[(Some(start), s), (Some(start + 20_000_000), late)],
        );
        assert!(r.problem.is_none(), "{r:?}");
        // The late camera sets the time; the others wait for it.
        assert_eq!(r.video[1].suggested_ms, 0);
        assert!((r.video[0].suggested_ms as i32 - 133).abs() <= 34, "{r:?}");
        assert!((r.audio[0].suggested_ms as i32 - r.video[0].suggested_ms as i32).abs() <= 34, "{r:?}");
        // The far mic heard it 40 ms late: it needs 40 ms less delay.
        assert!((r.audio[0].suggested_ms as i32 - r.audio[1].suggested_ms as i32 - 40).abs() <= 2, "{r:?}");
        // Nothing heard: no suggestions.
        let quiet = analyse(&[("Cam".into(), 0)], &frames[..1], &[("Mic".into(), 0)], &[(Some(start), vec![0.0; 48_000])]);
        assert!(quiet.problem.is_some() && quiet.video[0].suggested_ms == 0);
    }
}
