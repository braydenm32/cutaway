//! Windows: Media Foundation cameras and capture cards, encoders and
//! decoders (hardware H.264/HEVC on NVIDIA, AMD and Intel; AAC), Windows
//! Graphics Capture for screens. All part of Windows 11, so nothing needs
//! installing.

pub mod appaudio;
pub mod camera;
pub mod mf;
pub mod screen;
pub mod vcam;

pub use camera::{list_video_devices, open_camera};
pub use screen::{capture_targets, open_screen};

use anyhow::{anyhow, Result};
use std::sync::OnceLock;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_FULL};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Media Foundation's version (MF_SDK_VERSION << 16 | MF_API_VERSION).
const MF_VERSION: u32 = (0x0002 << 16) | 0x0070;

/// Started from a terminal: send printed output there. (Cutaway is a
/// windowed app, so Windows doesn't connect it to one by itself.)
pub fn attach_console() {
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::Console::{AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    // SAFETY: plain Win32 calls; the console handle stays open.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        if let Ok(h) = CreateFileW(windows::core::w!("CONOUT$"), GENERIC_WRITE.0, FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
            let _ = SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
    println!();
}

/// Windows asks for camera and microphone access through its privacy
/// settings, not a prompt: nothing to do up front.
pub fn request_permissions() {}

pub fn permissions_changed() -> bool {
    false
}

/// Join the multithreaded COM apartment (every thread that touches Media
/// Foundation or WinRT). Harmless if already done.
pub fn com() {
    // SAFETY: plain COM initialisation; a thread already in another
    // apartment just keeps it.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// Start Media Foundation (once per process) and join COM on this thread.
pub fn mf() -> Result<()> {
    com();
    static STARTED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            // SAFETY: plain call; stays started for the life of the process.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| format!("Media Foundation didn't start ({e})"))
        })
        .clone()
        .map_err(|e| anyhow!(e))
}

/// A Windows error in words, with what to do about the common ones.
pub(crate) fn explain(e: &windows::core::Error, what: &str) -> anyhow::Error {
    if e.code() == E_ACCESSDENIED {
        return anyhow!(
            "{what}: Windows blocked access. Turn on Settings → Privacy & security → Camera (or Microphone) → \"Let desktop apps access…\""
        );
    }
    anyhow!("{what}: {} ({:#010x})", e.message().trim(), e.code().0 as u32)
}

/// Text of a wide string from Media Foundation (and free it).
pub(crate) fn take_pwstr(p: windows::core::PWSTR, len: u32) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: Media Foundation returned a buffer of `len` UTF-16 units that
    // we own and must free with CoTaskMemFree.
    let s = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len as usize)) };
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

use windows::core::{Interface, GUID};
use windows::Win32::Media::MediaFoundation::{IMF2DBuffer2, IMFAttributes, IMFSample, MF2DBuffer_LockFlags_Read, MFCreateAttributes};

/// A new attribute store.
pub(crate) fn attributes(n: u32) -> Result<IMFAttributes> {
    let mut a = None;
    // SAFETY: valid out pointer.
    unsafe { MFCreateAttributes(&mut a, n) }.map_err(|e| explain(&e, "Media Foundation"))?;
    a.ok_or_else(|| anyhow!("Media Foundation gave no attribute store"))
}

/// Two 32-bit values in one attribute (frame sizes and rates).
pub(crate) fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

pub(crate) fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// A string attribute, if set.
pub(crate) fn string_attr(a: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut p = windows::core::PWSTR::null();
    let mut len = 0u32;
    // SAFETY: valid out pointers; the string is freed by take_pwstr.
    unsafe { a.GetAllocatedString(key, &mut p, &mut len) }.ok()?;
    Some(take_pwstr(p, len))
}

/// A COM object moved to another thread. Media Foundation objects live in
/// the multithreaded apartment, so this is fine for them.
pub(crate) struct Mta<T>(pub T);
// SAFETY: only used for free-threaded (MTA) COM objects.
unsafe impl<T> Send for Mta<T> {}

/// Copy an NV12 picture out of a locked buffer: `scan0` is the first luma
/// row, `pitch` the bytes per row, `len` the bytes from `scan0` on.
pub(crate) fn copy_nv12(scan0: *const u8, pitch: usize, len: usize, w: usize, h: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    if pitch < w || h < 2 {
        return None;
    }
    // The luma plane may be padded (1088 rows for 1080): work it out from
    // the buffer's size (it holds 1.5 × the padded rows).
    let needs = |padded: usize| pitch * (padded + h / 2 - 1) + w;
    let rows = len / pitch;
    let padded = (rows * 2 / 3).max(h);
    let padded = if needs(padded) <= len { padded } else { h };
    if needs(padded) > len {
        return None;
    }
    let mut y = vec![0u8; w * h];
    let mut uv = vec![0u8; w * h / 2];
    // SAFETY: every row read is inside the `len` bytes checked above.
    unsafe {
        for r in 0..h {
            std::ptr::copy_nonoverlapping(scan0.add(r * pitch), y.as_mut_ptr().add(r * w), w);
        }
        let base = scan0.add(pitch * padded);
        for r in 0..h / 2 {
            std::ptr::copy_nonoverlapping(base.add(r * pitch), uv.as_mut_ptr().add(r * w), w);
        }
    }
    Some((y, uv))
}

/// The NV12 picture in a sample.
pub(crate) fn planes(sample: &IMFSample, w: u32, h: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    let (w, h) = (w as usize, h as usize);
    // SAFETY: buffers are locked for reading and unlocked before returning.
    unsafe {
        let buf = sample.GetBufferByIndex(0).ok()?;
        if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
            let (mut scan0, mut pitch, mut start, mut len) = (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
            if b2.Lock2DSize(MF2DBuffer_LockFlags_Read, &mut scan0, &mut pitch, &mut start, &mut len).is_ok() {
                let avail = (start as usize + len as usize).saturating_sub(scan0 as usize);
                let out = if pitch > 0 { copy_nv12(scan0, pitch as usize, avail, w, h) } else { None };
                let _ = b2.Unlock2D();
                if out.is_some() {
                    return out;
                }
            }
        }
        let buf = sample.ConvertToContiguousBuffer().ok()?;
        let (mut p, mut cur) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut cur)).ok()?;
        let out = copy_nv12(p, w, cur as usize, w, h);
        let _ = buf.Unlock();
        out
    }
}
