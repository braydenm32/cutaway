//! The podcast copy of a take: the recording mix (from its WAV), brought
//! to a standard loudness with a look-ahead limiter, encoded as AAC in an
//! `.m4a` with the markers as chapters and the episode's tags.

use super::dsp::{LookaheadLimiter, LoudnessMeter};
use super::encode;
use crate::config::PodcastConfig;
use crate::media::mp4::{Brand, Mp4Writer};
use crate::media::wav::WavReader;
use crate::media::{Ns, AAC_FRAME, NS_PER_SEC};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// What a finished podcast file measured.
#[derive(Debug, Clone)]
pub struct PodcastReport {
    pub size: u64,
    /// Loudness of the mix as recorded, and the gain applied.
    pub measured_lufs: f64,
    pub gain_db: f64,
    pub seconds: f64,
}

/// Tags for the file.
pub struct Episode {
    pub title: String,
    pub show: String,
    pub author: String,
    /// "YYYY-MM-DD".
    pub date: String,
}

const CHUNK: usize = 48_000;

/// Make `out` from the mix WAV. `progress` gets 0..1.
pub fn make(wav: &Path, out: &Path, backup: Option<&Path>, cfg: &PodcastConfig, episode: &Episode, chapters: &[(Ns, String)], progress: &mut dyn FnMut(f32)) -> Result<PodcastReport> {
    // Pass 1: how loud is it?
    let mut r = WavReader::open(wav)?;
    if r.rate != 48_000 || r.channels != 2 {
        bail!("the mix WAV should be 48 kHz stereo");
    }
    let total = r.frames.max(1);
    let mut meter = LoudnessMeter::new();
    let mut done = 0u64;
    loop {
        let block = r.read(CHUNK)?;
        if block.is_empty() {
            break;
        }
        meter.push(&block);
        done += (block.len() / 2) as u64;
        progress(done as f32 / total as f32 * 0.3);
    }
    let measured = meter.integrated();
    // Silence (or nearly): leave it alone rather than amplify noise.
    let gain_db = if measured < -60.0 { 0.0 } else { (cfg.target_lufs.clamp(-30.0, -10.0) - measured).clamp(-20.0, 20.0) };
    let gain = 10f64.powf(gain_db / 20.0) as f32;

    // Pass 2: level, limit, encode.
    let channels: u16 = if cfg.mono { 1 } else { 2 };
    let bitrate = cfg.bitrate_kbps.clamp(48, 320);
    let mut aac = encode::create_aac(48_000, channels, bitrate)?;
    let acfg = aac.config();
    let mut w = Mp4Writer::create_with_backup(out, backup, Brand::Mp4)?;
    w.set_major_brand(*b"M4A ");
    let track = w.add_audio(&acfg);
    let mut tags = vec![(*b"\xa9too", format!("Cutaway {}", env!("CARGO_PKG_VERSION"))), (*b"\xa9gen", "Podcast".to_string())];
    for (tag, value) in [(*b"\xa9nam", &episode.title), (*b"\xa9alb", &episode.show), (*b"\xa9ART", &episode.author), (*b"\xa9day", &episode.date)] {
        if !value.trim().is_empty() {
            tags.push((tag, value.trim().to_string()));
        }
    }
    w.set_metadata(tags);
    w.set_chapters(chapters.to_vec());
    let mut limiter = LookaheadLimiter::new(-1.5, 5.0);
    let mut n = 0i64;
    let mut write = |w: &mut Mp4Writer, frames: Vec<Vec<u8>>| -> Result<()> {
        for f in frames {
            w.write(track, n * AAC_FRAME as i64 * NS_PER_SEC / 48_000, &f, true)?;
            n += 1;
        }
        Ok(())
    };
    let shape = |buf: Vec<f32>| -> Vec<f32> {
        if channels == 1 {
            buf.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
        } else {
            buf
        }
    };
    let mut r = WavReader::open(wav)?;
    let mut done = 0u64;
    loop {
        let mut block = r.read(CHUNK)?;
        if block.is_empty() {
            break;
        }
        done += (block.len() / 2) as u64;
        for s in &mut block {
            *s *= gain;
        }
        let out = shape(limiter.process(&block));
        write(&mut w, aac.push(&out)?)?;
        progress(0.3 + done as f32 / total as f32 * 0.7);
    }
    let tail = shape(limiter.flush());
    write(&mut w, aac.push(&tail)?)?;
    write(&mut w, aac.finish()?)?;
    let seconds = total as f64 / 48_000.0;
    w.set_duration(track, (seconds * NS_PER_SEC as f64) as Ns);
    let size = w.finish().context("writing the podcast file")?;
    progress(1.0);
    Ok(PodcastReport { size, measured_lufs: measured, gain_db, seconds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::wav::{SampleFormat, WavWriter};

    #[test]
    fn levels_the_mix_and_keeps_chapters() {
        let dir = std::env::temp_dir().join(format!("cutaway-podcast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("Mix.wav");
        // A quiet 1 kHz "voice" at -30 dBFS with pauses, 6 seconds.
        let mut w = WavWriter::create(&wav, SampleFormat::I24, 2, 48000).unwrap();
        let amp = 10f32.powf(-30.0 / 20.0);
        for sec in 0..6 {
            let on = sec % 3 != 2;
            let block: Vec<f32> = (0..96_000).map(|i| if on { amp * ((i / 2) as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin() } else { 0.0 }).collect();
            w.write(&block).unwrap();
        }
        w.finish().unwrap();
        let out = dir.join("Take (Podcast).m4a");
        let cfg = PodcastConfig { enabled: true, ..Default::default() };
        let ep = Episode { title: "Episode 12".into(), show: "The Show".into(), author: "Host".into(), date: "2026-10-18".into() };
        let mut last = 0.0;
        let rep = make(&wav, &out, None, &cfg, &ep, &[(2_000_000_000, "Guest arrives".into())], &mut |p| last = p).unwrap();
        assert_eq!(last, 1.0);
        assert!((rep.measured_lufs - -30.0).abs() < 1.0, "{}", rep.measured_lufs);
        assert!((rep.gain_db - 14.0).abs() < 1.0, "{}", rep.gain_db);
        assert!((rep.seconds - 6.0).abs() < 0.01);
        if let Ok(o) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration,format_name:format_tags=title,album,artist:chapter=start_time:chapter_tags=title", "-of", "compact"])
            .arg(&out)
            .output()
        {
            let s = String::from_utf8_lossy(&o.stdout);
            assert!(s.contains("Guest arrives") && s.contains("title=Episode 12") && s.contains("album=The Show"), "{s}");
            assert!(s.contains("duration=6.0"), "{s}");
            // Measure the result: close to -16 LUFS.
            if let Ok(o) = std::process::Command::new("ffmpeg").args(["-hide_banner", "-nostats", "-i"]).arg(&out).args(["-af", "ebur128", "-f", "null", "-"]).output() {
                let e = String::from_utf8_lossy(&o.stderr);
                let i = e.rsplit("I:").next().and_then(|t| t.split_whitespace().next()).and_then(|v| v.parse::<f64>().ok());
                assert!(i.is_some_and(|i| (i - -16.0).abs() < 1.0), "{i:?}");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
