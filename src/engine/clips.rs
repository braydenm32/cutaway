//! Instant clips: while recording, the last stretch of the program (as
//! encoded for its file) is kept in memory, and a button saves it as its
//! own file in the take's "Clips" folder — for social media, a highlight,
//! a blooper — while the recording carries on.

use crate::util::LockExt;
use super::recorder::MuxMsg;
use crate::media::mp4::{Brand, Mp4Writer};
use crate::media::{AudioConfig, Ns, VideoPacket, NS_PER_SEC};
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Memory the kept stretch may take at most, whatever its length: ProRes
/// at 4K is close to 100 MB a second.
const MAX_BYTES: usize = 2 << 30;

#[derive(Default)]
struct Inner {
    /// The take being recorded (None between takes).
    folder: Option<PathBuf>,
    fps: u32,
    brand: Option<Brand>,
    audio_cfg: Option<AudioConfig>,
    keep: Ns,
    video: VecDeque<VideoPacket>,
    /// Size of `video`'s pictures, and the most it may be.
    bytes: usize,
    max_bytes: usize,
    audio: VecDeque<(Ns, Arc<Vec<u8>>)>,
    saved: u32,
}

#[derive(Default)]
pub struct ClipBuffer {
    inner: Mutex<Inner>,
}

/// A clip that was saved.
#[derive(Clone, Debug)]
pub struct Clip {
    pub path: PathBuf,
    pub seconds: f64,
}

impl ClipBuffer {
    pub fn new() -> Arc<ClipBuffer> {
        Arc::new(ClipBuffer::default())
    }

    /// A take started: keep the last `keep_seconds` of its program.
    pub fn start(&self, folder: PathBuf, fps: u32, brand: Brand, audio_cfg: Option<AudioConfig>, keep_seconds: u32) {
        let mut i = self.inner.lock_ok();
        *i = Inner { folder: Some(folder), fps, brand: Some(brand), audio_cfg, keep: (keep_seconds.max(5) as Ns + 4) * NS_PER_SEC, max_bytes: MAX_BYTES, ..Default::default() };
    }

    /// The take ended.
    pub fn stop(&self) {
        let mut i = self.inner.lock_ok();
        i.folder = None;
        i.video.clear();
        i.bytes = 0;
        i.audio.clear();
    }

    /// What the Program file is getting.
    pub fn push(&self, msg: &MuxMsg) {
        let mut i = self.inner.lock_ok();
        if i.folder.is_none() {
            return;
        }
        match msg {
            MuxMsg::Video(p) => {
                i.bytes += p.data.len();
                i.video.push_back(p.clone());
            }
            MuxMsg::Audio(t, d) => i.audio.push_back((*t, d.clone())),
            _ => return,
        }
        // Forget what's too old (or too much), keeping a keyframe to start from.
        let newest = i.video.back().map(|p| p.dts).unwrap_or(0);
        let (keep, max) = (i.keep, i.max_bytes);
        while i.video.len() > 1 && (newest - i.video[0].dts > keep || i.bytes > max) {
            let next_key = i.video.iter().skip(1).position(|p| p.keyframe);
            match next_key {
                Some(k) if newest - i.video[k + 1].dts > keep || i.bytes > max => {
                    let gone: usize = i.video.drain(..=k).map(|p| p.data.len()).sum();
                    i.bytes -= gone;
                }
                _ => break,
            }
        }
        let oldest = i.video.front().map(|p| p.dts).unwrap_or(newest);
        while i.audio.front().is_some_and(|a| a.0 < oldest - NS_PER_SEC) {
            i.audio.pop_front();
        }
    }

    /// The last `seconds` of the program's pictures (from the keyframe
    /// before), for an instant replay.
    pub fn recent(&self, seconds: f32) -> Result<Vec<VideoPacket>> {
        let i = self.inner.lock_ok();
        if i.folder.is_none() {
            return Err(anyhow!("replays come from the recording: start recording first"));
        }
        let newest = i.video.back().map(|p| p.dts).ok_or_else(|| anyhow!("nothing recorded yet"))?;
        let want = newest - (seconds.max(0.5) * 1e9) as Ns;
        let start = i.video.iter().rposition(|p| p.keyframe && p.dts <= want).or_else(|| i.video.iter().position(|p| p.keyframe)).ok_or_else(|| anyhow!("nothing recorded yet"))?;
        Ok(i.video.iter().skip(start).cloned().collect())
    }

    /// The next clip's number (errors when not recording).
    pub fn reserve(&self) -> Result<u32> {
        let mut i = self.inner.lock_ok();
        if i.folder.is_none() {
            return Err(anyhow!("clips are saved from a recording: start recording first"));
        }
        i.saved += 1;
        Ok(i.saved)
    }

    /// Save the last `seconds` (or as much as there is) as clip `number`.
    /// Starts at a keyframe, so it can be up to a couple of seconds longer.
    pub fn save(&self, seconds: u32, number: u32) -> Result<Clip> {
        let (folder, fps, brand, audio_cfg, video, audio) = {
            let i = self.inner.lock_ok();
            let folder = i.folder.clone().ok_or_else(|| anyhow!("clips are saved from a recording: start recording first"))?;
            let newest = i.video.back().map(|p| p.dts).ok_or_else(|| anyhow!("nothing recorded yet"))?;
            let want = newest - seconds.max(1) as Ns * NS_PER_SEC;
            let start = i.video.iter().rposition(|p| p.keyframe && p.dts <= want).or_else(|| i.video.iter().position(|p| p.keyframe)).ok_or_else(|| anyhow!("nothing recorded yet"))?;
            let video: Vec<VideoPacket> = i.video.iter().skip(start).cloned().collect();
            let t0 = video[0].dts;
            let audio: Vec<(Ns, Arc<Vec<u8>>)> = i.audio.iter().filter(|a| a.0 >= t0).cloned().collect();
            (folder, i.fps, i.brand.unwrap_or(Brand::Mp4), i.audio_cfg.clone(), video, audio)
        };
        let t0 = video[0].dts;
        let end = video.last().map(|p| p.dts).unwrap_or(t0) + NS_PER_SEC / fps.max(1) as Ns;
        let dir = folder.join("Clips");
        std::fs::create_dir_all(&dir)?;
        let at = t0.max(0) / NS_PER_SEC;
        let ext = if brand == Brand::Mov { "mov" } else { "mp4" };
        let path = dir.join(format!("Clip {number:02} ({}m{:02}s).{ext}", at / 60, at % 60));
        let mut w = Mp4Writer::create(&path, brand)?;
        let v = w.add_video(&video[0].config, fps);
        let a = audio_cfg.as_ref().map(|c| w.add_audio(c));
        // Interleaved by time, as players like.
        let mut ai = 0;
        for p in &video {
            if let Some(a) = a {
                while ai < audio.len() && audio[ai].0 <= p.dts {
                    w.write(a, audio[ai].0 - t0, &audio[ai].1, true)?;
                    ai += 1;
                }
            }
            w.write(v, p.dts - t0, &p.data, p.keyframe)?;
        }
        if let Some(a) = a {
            while ai < audio.len() && audio[ai].0 < end {
                w.write(a, audio[ai].0 - t0, &audio[ai].1, true)?;
                ai += 1;
            }
        }
        w.finish()?;
        Ok(Clip { path, seconds: (end - t0) as f64 / 1e9 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{VideoCodec, VideoConfig};

    fn packet(i: i64, fps: i64, cfg: &Arc<VideoConfig>) -> VideoPacket {
        let t = i * NS_PER_SEC / fps;
        VideoPacket { pts: t, dts: t, keyframe: i % (fps * 2) == 0, data: Arc::new(vec![0, 0, 0, 2, 0x09, 0x10]), config: cfg.clone() }
    }

    #[test]
    fn keeps_the_last_stretch_and_saves_it() {
        let dir = std::env::temp_dir().join(format!("cutaway-clips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Arc::new(VideoConfig { codec: VideoCodec::H264, width: 64, height: 36, record: vec![1, 0x64, 0, 0x1f, 0xff, 0xe0, 0, 0xe1, 0], parameter_sets: Vec::new() });
        let b = ClipBuffer::new();
        assert!(b.reserve().is_err(), "not recording");
        b.start(dir.clone(), 30, Brand::Mp4, None, 10);
        // 60 seconds at 30 fps, keyframe every 2 s.
        for i in 0..1800 {
            b.push(&MuxMsg::Video(packet(i, 30, &cfg)));
        }
        let kept = b.inner.lock_ok().video.len();
        assert!((14 * 30..=17 * 30).contains(&kept), "kept {kept} frames (about 14 s)");
        let n = b.reserve().unwrap();
        let clip = b.save(10, n).unwrap();
        // The newest frame is at 59.97 s; the keyframe before 49.97 s is at 48 s.
        assert!((clip.seconds - 12.0).abs() < 0.1, "{} s", clip.seconds);
        assert!(clip.path.ends_with("Clips/Clip 01 (0m48s).mp4"), "{}", clip.path.display());
        assert!(std::fs::metadata(&clip.path).unwrap().len() > 1000);
        let second = b.save(5, b.reserve().unwrap()).unwrap();
        assert!(second.path.ends_with("Clips/Clip 02 (0m54s).mp4"), "{}", second.path.display());
        b.stop();
        assert!(b.reserve().is_err());
        assert!(b.save(5, 3).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Big pictures (ProRes) are kept by size as well as by length.
    #[test]
    fn keeps_no_more_than_its_memory() {
        let cfg = Arc::new(VideoConfig { codec: VideoCodec::H264, width: 64, height: 36, record: vec![1, 0x64, 0, 0x1f, 0xff, 0xe0, 0, 0xe1, 0], parameter_sets: Vec::new() });
        let b = ClipBuffer::new();
        b.start(std::env::temp_dir(), 30, Brand::Mov, None, 600);
        b.inner.lock_ok().max_bytes = 6 * 300;
        for i in 0..1800 {
            b.push(&MuxMsg::Video(packet(i, 30, &cfg)));
        }
        let i = b.inner.lock_ok();
        assert!(i.bytes <= 6 * 300 && i.bytes == i.video.len() * 6, "{} bytes", i.bytes);
        assert!(i.video.len() >= 240 && i.video[0].keyframe, "kept {}", i.video.len());
    }
}
