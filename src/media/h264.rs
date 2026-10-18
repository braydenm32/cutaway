//! Bitstream helpers: Annex-B ⇄ AVCC, `avcC` records, AAC configs.

// Bitstream helpers; which ones are used depends on the platform's encoders.
#![allow(dead_code)]

/// Split an Annex-B byte stream (00 00 01 / 00 00 00 01 start codes) into
/// NAL units (without start codes).
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                let mut end = i;
                // A 4-byte start code's leading zero belongs to it, not the NAL.
                while end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                if end > s {
                    nals.push(&data[s..end]);
                }
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        if s < data.len() {
            nals.push(&data[s..]);
        }
    }
    nals
}

/// Split AVCC (4-byte length prefixed) data into NAL units.
pub fn split_avcc(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        i += 4;
        if i + len > data.len() {
            break;
        }
        out.push(&data[i..i + len]);
        i += len;
    }
    out
}

pub fn h264_nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| b & 0x1f).unwrap_or(0)
}

pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;
pub const NAL_SEI: u8 = 6;

/// Write NAL units as AVCC.
pub fn to_avcc<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nals {
        out.extend_from_slice(&(n.len() as u32).to_be_bytes());
        out.extend_from_slice(n);
    }
    out
}

/// Write NAL units as Annex-B.
pub fn to_annexb<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(n);
    }
    out
}

/// AVCDecoderConfigurationRecord from one SPS and one PPS.
pub fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut r = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    r.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    r.extend_from_slice(sps);
    r.push(1);
    r.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    r.extend_from_slice(pps);
    // High profiles carry chroma/bit-depth fields (4:2:0, 8-bit).
    if matches!(sps[1], 100 | 110 | 122 | 144) {
        r.extend_from_slice(&[0xFC | 1, 0xF8, 0xF8, 0]);
    }
    r
}

/// SPS and PPS units from an `avcC` record.
pub fn parse_avcc(rec: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if rec.len() < 7 {
        return out;
    }
    let mut i = 5;
    let nsps = (rec[i] & 0x1f) as usize;
    i += 1;
    for _ in 0..nsps {
        if i + 2 > rec.len() {
            return out;
        }
        let len = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
        i += 2;
        if i + len > rec.len() {
            return out;
        }
        out.push(rec[i..i + len].to_vec());
        i += len;
    }
    if i >= rec.len() {
        return out;
    }
    let npps = rec[i] as usize;
    i += 1;
    for _ in 0..npps {
        if i + 2 > rec.len() {
            return out;
        }
        let len = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
        i += 2;
        if i + len > rec.len() {
            return out;
        }
        out.push(rec[i..i + len].to_vec());
        i += len;
    }
    out
}

/// Parameter sets (VPS, SPS, PPS) from an `hvcC` record.
pub fn parse_hvcc(rec: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if rec.len() < 23 {
        return out;
    }
    let mut i = 22;
    let arrays = rec[i] as usize;
    i += 1;
    for _ in 0..arrays {
        if i + 3 > rec.len() {
            return out;
        }
        let count = u16::from_be_bytes([rec[i + 1], rec[i + 2]]) as usize;
        i += 3;
        for _ in 0..count {
            if i + 2 > rec.len() {
                return out;
            }
            let len = u16::from_be_bytes([rec[i], rec[i + 1]]) as usize;
            i += 2;
            if i + len > rec.len() {
                return out;
            }
            out.push(rec[i..i + len].to_vec());
            i += len;
        }
    }
    out
}

/// HEVC NAL unit type.
pub fn hevc_nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| (b >> 1) & 0x3f).unwrap_or(0)
}

pub const HEVC_VPS: u8 = 32;
pub const HEVC_SPS: u8 = 33;
pub const HEVC_PPS: u8 = 34;
pub const HEVC_AUD: u8 = 35;

/// A NAL unit's payload without emulation-prevention bytes (00 00 03 → 00 00).
pub fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Reads bits and Exp-Golomb codes.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn u(&mut self, n: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u64;
            self.pos += 1;
        }
        Some(v)
    }

    fn ue(&mut self) -> Option<u64> {
        let mut zeros = 0;
        while self.u(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u64 << zeros) - 1 + self.u(zeros)?)
    }
}

/// HEVCDecoderConfigurationRecord (`hvcC`) from a VPS, SPS and PPS
/// (without start codes), for 4-byte NAL lengths.
pub fn build_hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let rbsp = unescape(sps.get(2..).unwrap_or(&[]));
    let mut b = Bits { data: &rbsp, pos: 0 };
    let mut ptl = [0u8; 12];
    let mut sub_layers = 0u64;
    let mut nested = 1u64;
    let mut chroma = 1u64;
    let (mut depth_luma, mut depth_chroma) = (0u64, 0u64);
    (|| -> Option<()> {
        b.u(4)?; // sps_video_parameter_set_id
        sub_layers = b.u(3)?;
        nested = b.u(1)?;
        for byte in ptl.iter_mut() {
            *byte = b.u(8)? as u8;
        }
        let mut profile_present = [false; 8];
        let mut level_present = [false; 8];
        for i in 0..sub_layers as usize {
            profile_present[i] = b.u(1)? == 1;
            level_present[i] = b.u(1)? == 1;
        }
        if sub_layers > 0 {
            for _ in sub_layers..8 {
                b.u(2)?;
            }
        }
        for i in 0..sub_layers as usize {
            if profile_present[i] {
                b.u(32)?;
                b.u(32)?;
                b.u(24)?;
            }
            if level_present[i] {
                b.u(8)?;
            }
        }
        b.ue()?; // sps_seq_parameter_set_id
        chroma = b.ue()?;
        if chroma == 3 {
            b.u(1)?;
        }
        b.ue()?; // width
        b.ue()?; // height
        if b.u(1)? == 1 {
            for _ in 0..4 {
                b.ue()?;
            }
        }
        depth_luma = b.ue()?;
        depth_chroma = b.ue()?;
        Some(())
    })();
    let mut r = vec![1];
    r.extend_from_slice(&ptl); // profile space/tier/profile, compatibility, constraints, level
    r.extend_from_slice(&[0xF0, 0x00, 0xFC]); // min_spatial_segmentation 0, parallelism unknown
    r.push(0xFC | (chroma as u8 & 3));
    r.push(0xF8 | (depth_luma as u8 & 7));
    r.push(0xF8 | (depth_chroma as u8 & 7));
    r.extend_from_slice(&[0, 0]); // average frame rate unknown
    r.push((((sub_layers as u8 + 1) & 7) << 3) | ((nested as u8 & 1) << 2) | 3);
    r.push(3);
    for (t, nal) in [(HEVC_VPS, vps), (HEVC_SPS, sps), (HEVC_PPS, pps)] {
        r.push(0x80 | t); // complete: parameter sets only here
        r.extend_from_slice(&1u16.to_be_bytes());
        r.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        r.extend_from_slice(nal);
    }
    r
}

/// An encoder's output (an Annex-B access unit) as a packet. Parameter
/// sets are taken out and kept in `config` (renewed when they change);
/// `header` is where the encoder put them if not in the stream. Nothing
/// comes out until the parameter sets are known.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn packet_from_annexb(
    codec: super::VideoCodec,
    size: (u32, u32),
    annexb: &[u8],
    header: Option<&[u8]>,
    pts: super::Ns,
    config: &mut Option<std::sync::Arc<super::VideoConfig>>,
) -> Option<super::VideoPacket> {
    use super::{VideoCodec, VideoConfig, VideoPacket};
    let hevc = codec == VideoCodec::Hevc;
    let nals = split_annexb(annexb);
    let header_nals = header.map(split_annexb).unwrap_or_default();
    let ty = |n: &[u8]| if hevc { hevc_nal_type(n) } else { h264_nal_type(n) };
    let find = |t: u8| nals.iter().chain(header_nals.iter()).find(|n| ty(n) == t).copied();
    let rec = if hevc {
        match (find(HEVC_VPS), find(HEVC_SPS), find(HEVC_PPS)) {
            (Some(v), Some(s), Some(p)) => Some((build_hvcc(v, s, p), vec![v.to_vec(), s.to_vec(), p.to_vec()])),
            _ => None,
        }
    } else {
        match (find(NAL_SPS), find(NAL_PPS)) {
            (Some(s), Some(p)) => Some((build_avcc(s, p), vec![s.to_vec(), p.to_vec()])),
            _ => None,
        }
    };
    if let Some((record, parameter_sets)) = rec {
        if config.as_ref().is_none_or(|c| c.record != record) {
            *config = Some(std::sync::Arc::new(VideoConfig { codec, width: size.0, height: size.1, record, parameter_sets }));
        }
    }
    let config = config.clone()?;
    let keyframe = nals.iter().any(|n| if hevc { (16..=21).contains(&hevc_nal_type(n)) } else { h264_nal_type(n) == NAL_IDR });
    let body = to_avcc(nals.iter().copied().filter(|n| {
        let t = ty(n);
        if hevc {
            !matches!(t, HEVC_VPS | HEVC_SPS | HEVC_PPS | HEVC_AUD)
        } else {
            !matches!(t, NAL_SPS | NAL_PPS | NAL_AUD)
        }
    }));
    if body.is_empty() {
        return None;
    }
    Some(VideoPacket { pts, dts: pts, keyframe, data: std::sync::Arc::new(body), config })
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

fn rate_index(rate: u32) -> u8 {
    AAC_RATES.iter().position(|r| *r == rate).unwrap_or(3) as u8
}

/// AudioSpecificConfig for AAC-LC.
pub fn aac_asc(rate: u32, channels: u16) -> Vec<u8> {
    let aot: u16 = 2;
    let idx = rate_index(rate) as u16;
    let ch = channels.min(7);
    let v: u16 = (aot << 11) | (idx << 7) | (ch << 3);
    v.to_be_bytes().to_vec()
}

/// 7-byte ADTS header for one raw AAC-LC frame of `payload_len` bytes.
pub fn adts_header(rate: u32, channels: u16, payload_len: usize) -> [u8; 7] {
    let len = payload_len + 7;
    let profile = 1u8; // AAC-LC (object type 2) - 1
    let idx = rate_index(rate);
    let ch = channels.min(7) as u8;
    [
        0xFF,
        0xF1, // MPEG-4, layer 0, no CRC
        (profile << 6) | (idx << 2) | (ch >> 2),
        ((ch & 3) << 6) | ((len >> 11) as u8 & 0x3),
        ((len >> 3) & 0xFF) as u8,
        (((len & 7) << 5) as u8) | 0x1F,
        0xFC,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annexb_roundtrip() {
        let stream = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9, 0];
        let nals = split_annexb(&stream);
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9, 0][..]]);
        let avcc = to_avcc(nals.iter().copied());
        assert_eq!(split_avcc(&avcc), nals);
    }

    #[test]
    fn avcc_record() {
        let sps = [0x67, 0x64, 0x00, 0x28, 0xAC];
        let pps = [0x68, 0xEE, 0x3C];
        let rec = build_avcc(&sps, &pps);
        assert_eq!(parse_avcc(&rec), vec![sps.to_vec(), pps.to_vec()]);
    }

    #[test]
    fn hvcc_record_matches_ffmpeg() {
        // x265's parameter sets (320×240 Main), and the hvcC FFmpeg wrote for them.
        let hex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let vps = hex("40010c01ffff01600000030090000003000003003c959809");
        let sps = hex("42010101600000030090000003000003003ca00a080f165959a4932bc05a020000030002000003003c10");
        let pps = hex("4401c172b46240");
        let rec = build_hvcc(&vps, &sps, &pps);
        let ffmpeg = hex("0101600000009000000000003cf000fcfdf8f800000f");
        assert_eq!(&rec[..22], &ffmpeg[..], "header");
        assert_eq!(rec[22], 3);
        assert_eq!(parse_hvcc(&rec), vec![vps, sps, pps]);
    }

    #[test]
    fn packets_from_encoder_output() {
        use crate::media::VideoCodec;
        let mut cfg = None;
        // No parameter sets yet: nothing.
        assert!(packet_from_annexb(VideoCodec::H264, (64, 64), &[0, 0, 0, 1, 0x41, 1], None, 0, &mut cfg).is_none());
        // A keyframe with them in the stream.
        let au = [0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x67, 0x64, 0, 0x28, 0xAC, 0, 0, 0, 1, 0x68, 0xEE, 0x3C, 0, 0, 0, 1, 0x65, 9, 9];
        let p = packet_from_annexb(VideoCodec::H264, (64, 64), &au, None, 5, &mut cfg).unwrap();
        assert!(p.keyframe && p.pts == 5);
        assert_eq!(split_avcc(&p.data), vec![&[0x65, 9, 9][..]]);
        assert_eq!(p.config.parameter_sets.len(), 2);
        // Parameter sets from the encoder's header instead.
        let mut cfg2 = None;
        let header = [0, 0, 0, 1, 0x67, 0x64, 0, 0x28, 0xAC, 0, 0, 0, 1, 0x68, 0xEE, 0x3C];
        assert!(packet_from_annexb(VideoCodec::H264, (64, 64), &[0, 0, 0, 1, 0x65, 1], Some(&header), 0, &mut cfg2).is_some());
    }

    #[test]
    fn aac_config() {
        assert_eq!(aac_asc(48000, 2), vec![0x11, 0x90]);
        assert_eq!(aac_asc(44100, 2), vec![0x12, 0x10]);
        let h = adts_header(48000, 2, 100);
        assert_eq!(&h[..3], &[0xFF, 0xF1, 0x4C]);
        let len = (((h[3] & 3) as usize) << 11) | ((h[4] as usize) << 3) | ((h[5] >> 5) as usize);
        assert_eq!(len, 107);
    }
}
