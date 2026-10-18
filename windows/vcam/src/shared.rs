//! How Cutaway hands pictures to its virtual camera: one shared block of
//! memory (made by the camera, which runs inside Windows' camera service),
//! holding the newest 1080p NV12 picture behind a sequence number.
//!
//! Used by both the camera (`cutaway_vcam.dll`) and Cutaway itself.

#![allow(dead_code)]

/// The camera's COM class: {B257406E-938A-4EE5-8BED-30C90CC1AA49}.
pub const CLSID_TEXT: &str = "{B257406E-938A-4EE5-8BED-30C90CC1AA49}";
pub const CLSID_U128: u128 = 0xb257406e_938a_4ee5_8bed_30c90cc1aa49;

/// The shared memory's name (in the global namespace, so the camera
/// service and the user's Cutaway both see it).
pub const MAPPING: &str = "Global\\CutawayVirtualCamera";

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const FPS: u32 = 30;
pub const MAGIC: u32 = u32::from_le_bytes(*b"CVCM");
pub const HEADER: usize = 64;
pub const FRAME_BYTES: usize = (WIDTH * HEIGHT * 3 / 2) as usize;
pub const SIZE: usize = HEADER + FRAME_BYTES;

// Header fields (byte offsets).
const OFF_MAGIC: usize = 0;
const OFF_SEQ: usize = 8;
const OFF_WRITTEN_MS: usize = 16;

/// Write a picture (NV12, WIDTH×HEIGHT, tightly packed) into the block.
///
/// # Safety
/// `base` must point to SIZE writable bytes of the mapping.
pub unsafe fn write(base: *mut u8, y: &[u8], uv: &[u8], now_ms: u64) {
    use std::sync::atomic::{AtomicU64, Ordering};
    unsafe {
        let seq = &*(base.add(OFF_SEQ) as *const AtomicU64);
        let s = seq.load(Ordering::Relaxed);
        // Odd while writing.
        seq.store(s | 1, Ordering::Release);
        std::sync::atomic::fence(Ordering::Release);
        (base.add(OFF_MAGIC) as *mut u32).write_unaligned(MAGIC);
        let frame = base.add(HEADER);
        let ylen = (WIDTH * HEIGHT) as usize;
        std::ptr::copy_nonoverlapping(y.as_ptr(), frame, y.len().min(ylen));
        std::ptr::copy_nonoverlapping(uv.as_ptr(), frame.add(ylen), uv.len().min(FRAME_BYTES - ylen));
        (base.add(OFF_WRITTEN_MS) as *mut u64).write_unaligned(now_ms);
        std::sync::atomic::fence(Ordering::Release);
        seq.store((s | 1) + 1, Ordering::Release);
    }
}

/// Copy the newest picture out (NV12, FRAME_BYTES) if there is a fresh
/// one (written within `max_age_ms` of `now_ms`).
///
/// # Safety
/// `base` must point to SIZE readable bytes of the mapping.
pub unsafe fn read(base: *const u8, out: &mut [u8], now_ms: u64, max_age_ms: u64) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    unsafe {
        if (base.add(OFF_MAGIC) as *const u32).read_unaligned() != MAGIC {
            return false;
        }
        let seq = &*(base.add(OFF_SEQ) as *const AtomicU64);
        for _ in 0..4 {
            let before = seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::thread::yield_now();
                continue;
            }
            let written = (base.add(OFF_WRITTEN_MS) as *const u64).read_unaligned();
            if now_ms.saturating_sub(written) > max_age_ms {
                return false;
            }
            std::ptr::copy_nonoverlapping(base.add(HEADER), out.as_mut_ptr(), out.len().min(FRAME_BYTES));
            std::sync::atomic::fence(Ordering::Acquire);
            if seq.load(Ordering::Acquire) == before {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_a_picture() {
        let mut block = vec![0u64; SIZE / 8 + 1];
        let base = block.as_mut_ptr() as *mut u8;
        let mut out = vec![0u8; FRAME_BYTES];
        unsafe {
            assert!(!read(base, &mut out, 1000, 500), "nothing yet");
            let y = vec![200u8; (WIDTH * HEIGHT) as usize];
            let uv = vec![90u8; (WIDTH * HEIGHT / 2) as usize];
            write(base, &y, &uv, 1000);
            assert!(read(base, &mut out, 1200, 500));
            assert_eq!(out[0], 200);
            assert_eq!(out[FRAME_BYTES - 1], 90);
            assert!(!read(base, &mut out, 2000, 500), "stale");
        }
    }
}
