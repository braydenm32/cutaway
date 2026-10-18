//! Sound files (WAV, AIFF, MP3, AAC/M4A, FLAC, Ogg Vorbis, ALAC, CAF) for
//! sound pads and stinger sounds: read whole, as 48 kHz stereo.

use anyhow::{anyhow, bail, Context, Result};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Longest sound kept (10 minutes): pads are jingles and beds, not albums.
const MAX_SECONDS: usize = 600;

/// Extensions offered in file pickers.
pub const EXTENSIONS: &[&str] = &["wav", "wave", "aif", "aiff", "mp3", "m4a", "aac", "mp4", "flac", "ogg", "oga", "caf"];

/// The file as interleaved 48 kHz stereo samples.
pub fn load(path: &Path) -> Result<Vec<f32>> {
    let (rate, channels, pcm) = decode(path).with_context(|| format!("reading {}", path.display()))?;
    let stereo = to_stereo(&pcm, channels);
    Ok(if rate == 48_000 { stereo } else { resample_stereo(&stereo, rate) })
}

/// (sample rate, channels, interleaved samples) as stored.
fn decode(path: &Path) -> Result<(u32, usize, Vec<f32>)> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| anyhow!("not a sound file Cutaway can read ({e})"))?;
    let track = reader.first_track_known_codec(TrackType::Audio).ok_or_else(|| anyhow!("there's no sound in it"))?;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else { bail!("there's no sound in it") };
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| anyhow!("its sound format isn't supported ({e})"))?;
    let mut rate = params.sample_rate.unwrap_or(48_000);
    let mut channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2).max(1);
    let mut pcm: Vec<f32> = Vec::new();
    let mut chunk: Vec<f32> = Vec::new();
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(anyhow!("{e}")),
        };
        if packet.track_id != id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                rate = buf.spec().rate();
                channels = buf.spec().channels().count().max(1);
                chunk.clear();
                buf.copy_to_vec_interleaved(&mut chunk);
                pcm.extend_from_slice(&chunk);
            }
            // A damaged packet: skip it, as players do.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow!("{e}")),
        }
        if pcm.len() > MAX_SECONDS * rate as usize * channels {
            break;
        }
    }
    if pcm.is_empty() {
        bail!("there's no sound in it");
    }
    Ok((rate, channels, pcm))
}

/// Mono becomes both sides; more than two channels keep the first two.
fn to_stereo(pcm: &[f32], channels: usize) -> Vec<f32> {
    match channels {
        2 => pcm.to_vec(),
        1 => pcm.iter().flat_map(|s| [*s, *s]).collect(),
        n => pcm.chunks_exact(n).flat_map(|f| [f[0], f[1]]).collect(),
    }
}

/// Interleaved stereo at `rate` to 48 kHz.
pub fn resample_stereo(input: &[f32], rate: u32) -> Vec<f32> {
    let params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2).oversampling_factor(128).interpolation(SincInterpolationType::Cubic);
    let ratio = 48_000.0 / rate.max(1) as f64;
    let chunk = 1024;
    let Ok(mut rs) = Async::<f32>::new_sinc(ratio, 1.0, &params, chunk, 2, FixedAsync::Input) else { return input.to_vec() };
    let frames = input.len() / 2;
    // Pad the end so the tail comes out (the resampler holds some back).
    let delay = rs.output_delay();
    let mut padded = input.to_vec();
    padded.resize((frames + chunk * 2) * 2, 0.0);
    let mut out = Vec::with_capacity((frames as f64 * ratio) as usize * 2 + 4096);
    let mut buf = vec![0.0f32; (chunk as f64 * ratio * 1.2) as usize * 2 + 64];
    let mut pos = 0;
    while pos + chunk <= padded.len() / 2 {
        let Ok(inp) = InterleavedSlice::new(&padded[pos * 2..(pos + chunk) * 2], 2, chunk) else { break };
        let cap = buf.len() / 2;
        let Ok(mut outp) = InterleavedSlice::new_mut(&mut buf, 2, cap) else { break };
        match rs.process_into_buffer(&inp, &mut outp, None) {
            Ok((_, produced)) => out.extend_from_slice(&buf[..produced * 2]),
            Err(_) => break,
        }
        pos += chunk;
    }
    let want = (frames as f64 * ratio).round() as usize;
    let start = delay.min(out.len() / 2);
    let mut v: Vec<f32> = out[start * 2..].to_vec();
    v.truncate(want * 2);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit WAV of a 1 kHz tone (mono, 44.1 kHz) for 0.5 s.
    fn write_wav(path: &Path) {
        let rate = 44_100u32;
        let n = rate as usize / 2;
        let mut data = Vec::new();
        for i in 0..n {
            let s = ((i as f64 * 1000.0 * std::f64::consts::TAU / rate as f64).sin() * 16000.0) as i16;
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 2).to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(path, f).unwrap();
    }

    #[test]
    fn reads_a_wav_as_48k_stereo() {
        let path = std::env::temp_dir().join(format!("cutaway-sound-{}.wav", std::process::id()));
        write_wav(&path);
        let s = load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        // 0.5 s at 48 kHz, stereo.
        assert!((s.len() as i64 / 2 - 24_000).abs() <= 2, "{} frames", s.len() / 2);
        // Still a tone of the same strength, the same on both sides.
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 16000.0 / 32768.0).abs() < 0.03, "peak {peak}");
        assert!(s.chunks(2).all(|f| f[0] == f[1]));
        // About 1000 zero crossings per second per direction: 500 rising in 0.5 s.
        let rising = s.chunks(2).collect::<Vec<_>>().windows(2).filter(|w| w[0][0] < 0.0 && w[1][0] >= 0.0).count();
        assert!((495..=505).contains(&rising), "{rising} rising edges");
    }

    #[test]
    fn rejects_what_isnt_sound() {
        let path = std::env::temp_dir().join(format!("cutaway-notsound-{}.txt", std::process::id()));
        std::fs::write(&path, "hello").unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
