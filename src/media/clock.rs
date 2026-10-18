//! The host clock everything is timestamped against.
//!
//! On macOS this is `mach_absolute_time` in nanoseconds (CLOCK_UPTIME_RAW),
//! which is the same timeline AVFoundation, ScreenCaptureKit and CoreAudio
//! use for their timestamps, so captured media needs no conversion. On
//! Windows it's the performance counter, which Media Foundation, WASAPI and
//! Windows Graphics Capture use.

use super::Ns;

#[cfg(target_os = "macos")]
const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
#[cfg(all(unix, not(target_os = "macos")))]
const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

#[cfg(unix)]
pub fn now() -> Ns {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(CLOCK, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Windows: the performance counter (QPC) in nanoseconds — the timeline
/// Media Foundation, WASAPI and Windows Graphics Capture stamp media with
/// (in 100 ns units: see [`from_100ns`]).
#[cfg(windows)]
pub fn now() -> Ns {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: valid pointer; never fails on Windows XP and later.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut c = 0i64;
    // SAFETY: valid pointer.
    let _ = unsafe { QueryPerformanceCounter(&mut c) };
    (c as i128 * 1_000_000_000 / freq as i128) as Ns
}

/// A Windows media timestamp (100 ns units of the performance counter) on
/// the host clock.
#[cfg(windows)]
pub fn from_100ns(t: i64) -> Ns {
    t.saturating_mul(100)
}

/// Sleep until `t` (host clock). Returns immediately if it's in the past.
pub fn sleep_until(t: Ns) {
    let d = t - now();
    if d > 0 {
        std::thread::sleep(std::time::Duration::from_nanos(d as u64));
    }
}



#[cfg(test)]
mod tests {
    #[test]
    fn monotonic() {
        let a = super::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = super::now();
        assert!(b - a >= 4_000_000, "{a} {b}");
    }
}
