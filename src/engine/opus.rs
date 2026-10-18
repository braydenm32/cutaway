//! Opus, the audio codec of WebRTC: guests' voices in, their return feed
//! out. (A Rust translation of libopus, so nothing to install.)

use anyhow::{anyhow, Result};
use unsafe_libopus as op;

/// 48 kHz stereo Opus encoder for 20 ms frames.
pub struct Encoder {
    st: *mut op::OpusEncoder,
    pending: Vec<f32>,
}

// SAFETY: the encoder state is only used through &mut self.
unsafe impl Send for Encoder {}

pub const FRAME: usize = 960; // 20 ms at 48 kHz

impl Encoder {
    pub fn new(bitrate: i32) -> Result<Encoder> {
        let mut err = 0;
        // SAFETY: plain constructor; checked for null below.
        let st = unsafe { op::opus_encoder_create(48_000, 2, op::OPUS_APPLICATION_VOIP, &mut err) };
        if st.is_null() || err != op::OPUS_OK {
            return Err(anyhow!("couldn't start the Opus encoder ({err})"));
        }
        // SAFETY: st is a valid encoder.
        unsafe {
            op::opus_encoder_ctl!(st, op::OPUS_SET_BITRATE_REQUEST, bitrate);
            op::opus_encoder_ctl!(st, op::OPUS_SET_INBAND_FEC_REQUEST, 1);
            op::opus_encoder_ctl!(st, op::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 5);
        }
        Ok(Encoder { st, pending: Vec::with_capacity(FRAME * 4) })
    }

    /// Add interleaved stereo samples; returns finished 20 ms packets.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= FRAME * 2 {
            let mut buf = vec![0u8; 1500];
            // SAFETY: pending holds FRAME stereo frames; buf is big enough.
            let n = unsafe { op::opus_encode_float(self.st, self.pending.as_ptr(), FRAME as i32, buf.as_mut_ptr(), buf.len() as i32) };
            self.pending.drain(..FRAME * 2);
            if n > 0 {
                buf.truncate(n as usize);
                out.push(buf);
            }
        }
        out
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_encoder_create, freed once.
        unsafe { op::opus_encoder_destroy(self.st) }
    }
}

/// Opus decoder: packets in, 48 kHz stereo out.
pub struct Decoder {
    st: *mut op::OpusDecoder,
    buf: Vec<f32>,
}

// SAFETY: the decoder state is only used through &mut self.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Decoder> {
        let mut err = 0;
        // SAFETY: plain constructor; checked for null below.
        let st = unsafe { op::opus_decoder_create(48_000, 2, &mut err) };
        if st.is_null() || err != op::OPUS_OK {
            return Err(anyhow!("couldn't start the Opus decoder ({err})"));
        }
        Ok(Decoder { st, buf: vec![0.0; 5760 * 2] })
    }

    /// One packet (or None for a lost one) as interleaved stereo samples.
    pub fn decode(&mut self, packet: Option<&[u8]>) -> Vec<f32> {
        let (ptr, len) = match packet {
            Some(p) if !p.is_empty() => (p.as_ptr(), p.len() as i32),
            _ => (std::ptr::null(), 0),
        };
        // SAFETY: buf holds 120 ms of stereo (the most a packet can be).
        let n = unsafe { op::opus_decode_float(self.st, ptr, len, self.buf.as_mut_ptr(), 5760, 0) };
        if n <= 0 {
            return Vec::new();
        }
        self.buf[..n as usize * 2].to_vec()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_decoder_create, freed once.
        unsafe { op::opus_decoder_destroy(self.st) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut enc = Encoder::new(64_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        // 200 ms of a 440 Hz tone, in 10 ms blocks like the mixer's.
        let mut packets = Vec::new();
        for b in 0..20 {
            let block: Vec<f32> = (0..480).flat_map(|i| {
                let t = (b * 480 + i) as f32 / 48_000.0;
                let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                [v, v]
            }).collect();
            packets.extend(enc.push(&block));
        }
        assert_eq!(packets.len(), 10, "20 ms packets");
        let mut out = Vec::new();
        for p in &packets {
            out.extend(dec.decode(Some(p)));
        }
        assert_eq!(out.len(), 10 * FRAME * 2);
        // After the codec's start-up, the tone comes back at about the same level.
        let tail = &out[out.len() / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!((rms - 0.354).abs() < 0.08, "rms {rms}");
        assert_eq!(dec.decode(None).len() % 2, 0, "concealment for a lost packet");
    }
}
