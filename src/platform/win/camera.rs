//! Cameras and capture cards through Media Foundation: webcams and USB or
//! PCIe capture devices (anything the Windows Camera app can see).
//!
//! The device is set to the mode closest to the source's size and rate;
//! Media Foundation decodes MJPEG and converts to NV12 (and scales, when
//! the device has no exact mode; on the graphics card for MJPEG and other
//! compressed modes), and the frames are stamped on the host clock.

use super::{attributes, explain, mf, pack, planes, string_attr, unpack, Mta};
use crate::devices::{DeviceInfo, Mode};
use crate::engine::sources::Running;
use crate::media::{clock, Ns, VideoFrame, NS_PER_SEC};
use crate::platform::FrameSink;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{BOOL, GUID};
use windows::Win32::Media::MediaFoundation::*;

/// Every video capture device, as activation objects.
fn activations() -> Result<Vec<IMFActivate>> {
    mf()?;
    let a = attributes(1)?;
    // SAFETY: plain attribute calls on a valid store.
    unsafe { a.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID) }.map_err(|e| explain(&e, "Listing cameras"))?;
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    // SAFETY: valid out pointers.
    unsafe { MFEnumDeviceSources(&a, &mut list, &mut n) }.map_err(|e| explain(&e, "Listing cameras"))?;
    let mut out = Vec::new();
    if !list.is_null() {
        for i in 0..n as usize {
            // SAFETY: Media Foundation returned `n` activation objects that
            // we now own; each is moved out once, then the array is freed.
            if let Some(act) = unsafe { std::ptr::read(list.add(i)) } {
                out.push(act);
            }
        }
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(list as *const _)) };
    }
    Ok(out)
}

/// A mode the device offers.
struct Native {
    ty: IMFMediaType,
    subtype: GUID,
    width: u32,
    height: u32,
    fps: f64,
}

fn subtype_name(g: &GUID) -> String {
    let known = [
        (MFVideoFormat_NV12, "NV12"),
        (MFVideoFormat_YUY2, "YUY2"),
        (MFVideoFormat_UYVY, "UYVY"),
        (MFVideoFormat_MJPG, "MJPEG"),
        (MFVideoFormat_H264, "H.264"),
        (MFVideoFormat_RGB32, "RGB32"),
        (MFVideoFormat_RGB24, "RGB24"),
        (MFVideoFormat_I420, "I420"),
        (MFVideoFormat_P010, "P010"),
    ];
    known.iter().find(|(k, _)| k == g).map(|(_, n)| n.to_string()).unwrap_or_else(|| {
        // FourCC subtypes: the first four bytes are the code.
        let b = g.data1.to_le_bytes();
        if b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ') {
            String::from_utf8_lossy(&b).trim().to_string()
        } else {
            "other".into()
        }
    })
}

fn compressed(g: &GUID) -> bool {
    *g == MFVideoFormat_MJPG || *g == MFVideoFormat_H264 || *g == MFVideoFormat_HEVC
}

/// The device's first (capture) stream: its type handler and modes.
fn native_types(source: &IMFMediaSource) -> Result<(IMFMediaTypeHandler, Vec<Native>)> {
    // SAFETY: plain Media Foundation calls on valid objects.
    unsafe {
        let pd = source.CreatePresentationDescriptor()?;
        let mut selected = BOOL(0);
        let mut sd = None;
        pd.GetStreamDescriptorByIndex(0, &mut selected, &mut sd)?;
        let handler = sd.ok_or_else(|| anyhow!("the device has no video stream"))?.GetMediaTypeHandler()?;
        let mut out = Vec::new();
        for i in 0..handler.GetMediaTypeCount()? {
            let Ok(ty) = handler.GetMediaTypeByIndex(i) else { continue };
            let Ok(subtype) = ty.GetGUID(&MF_MT_SUBTYPE) else { continue };
            let Ok(size) = ty.GetUINT64(&MF_MT_FRAME_SIZE) else { continue };
            let (width, height) = unpack(size);
            let fps = ty.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).map(|(n, d)| n as f64 / d.max(1) as f64).unwrap_or(30.0);
            if width > 0 && height > 0 {
                out.push(Native { ty, subtype, width, height, fps });
            }
        }
        Ok((handler, out))
    }
}

fn modes(list: &[Native]) -> Vec<Mode> {
    let mut sizes: Vec<(u32, u32)> = list.iter().map(|n| (n.width, n.height)).collect();
    sizes.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
    sizes.dedup();
    sizes
        .into_iter()
        .map(|(w, h)| {
            let at: Vec<&Native> = list.iter().filter(|n| n.width == w && n.height == h).collect();
            let mut fps: Vec<u32> = at.iter().map(|n| n.fps.round() as u32).filter(|f| *f > 0).collect();
            fps.sort_unstable_by(|a, b| b.cmp(a));
            fps.dedup();
            Mode { width: w, height: h, fps, mjpeg_only: at.iter().all(|n| compressed(&n.subtype)) }
        })
        .collect()
}

pub fn list_video_devices() -> Vec<DeviceInfo> {
    let Ok(list) = activations() else { return Vec::new() };
    list.iter()
        .filter_map(|act| {
            let name = string_attr(act, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?;
            let id = string_attr(act, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).unwrap_or_else(|| name.clone());
            // Its modes (the device is opened briefly; one that's in use
            // by another app is listed without them).
            // SAFETY: plain Media Foundation calls.
            let modes = unsafe {
                match act.ActivateObject::<IMFMediaSource>() {
                    Ok(source) => {
                        let m = native_types(&source).map(|(_, l)| modes(&l)).unwrap_or_default();
                        let _ = source.Shutdown();
                        let _ = act.ShutdownObject();
                        m
                    }
                    Err(_) => Vec::new(),
                }
            };
            Some(DeviceInfo { summary: crate::devices::summarize(&modes), name, id, max_channels: 0, rate: 0, modes })
        })
        .collect()
}

/// The device mode to use for `w`×`h` at `fps`: the exact size if there
/// is one (else the smallest bigger one, else the biggest), the rate at or
/// just above `fps`, uncompressed before MJPEG.
fn pick(list: &[Native], w: u32, h: u32, fps: u32) -> Option<&Native> {
    let want = fps.max(1) as f64;
    let size_score = |n: &Native| -> u64 {
        if n.width == w && n.height == h {
            0
        } else if n.width >= w && n.height >= h {
            1 + (n.width as u64 * n.height as u64)
        } else {
            u64::MAX / 2 - (n.width as u64 * n.height as u64)
        }
    };
    let rate_score = |n: &Native| -> u64 {
        if n.fps + 0.5 >= want {
            ((n.fps - want).abs() * 100.0) as u64
        } else {
            1_000_000 + ((want - n.fps) * 100.0) as u64
        }
    };
    let format_score = |n: &Native| -> u64 {
        match () {
            _ if n.subtype == MFVideoFormat_NV12 => 0,
            _ if n.subtype == MFVideoFormat_YUY2 || n.subtype == MFVideoFormat_UYVY => 1,
            _ if compressed(&n.subtype) => 3,
            _ => 2,
        }
    };
    list.iter().min_by_key(|n| (size_score(n), rate_score(n), format_score(n)))
}

/// Media timestamps (100 ns) on the host clock.
struct TimeMap {
    offset: Option<Ns>,
}

impl TimeMap {
    fn host(&mut self, timestamp: i64, device_time: Option<u64>) -> Ns {
        let now = clock::now();
        // The capture moment on the performance counter, when the driver
        // says (Windows 10 and later).
        if let Some(t) = device_time.map(|t| clock::from_100ns(t as i64)) {
            if (t - now).abs() < 5 * NS_PER_SEC && t <= now {
                return t;
            }
        }
        let t = clock::from_100ns(timestamp);
        if (t - now).abs() < 2 * NS_PER_SEC && t <= now + NS_PER_SEC / 20 {
            return t.min(now);
        }
        // Timestamps from another zero: keep their spacing, anchored to
        // when the first frame arrived (re-anchored after a glitch).
        let off = *self.offset.get_or_insert(now - t);
        let mapped = t + off;
        if (mapped - now).abs() > NS_PER_SEC {
            self.offset = Some(now - t);
            return now;
        }
        mapped.min(now)
    }
}

struct Camera {
    stop: Arc<AtomicBool>,
    source: Mta<IMFMediaSource>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblocks a reader waiting for a frame.
        // SAFETY: shutting down a media source from any thread is allowed.
        let _ = unsafe { self.source.0.Shutdown() };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A source reader giving NV12 at `w`×`h` (or the device's own size, if
/// Windows can't scale it), and the size it gives. With `card`, decoding
/// and scaling happen on the graphics card.
fn open_reader(source: &IMFMediaSource, name: &str, w: u32, h: u32, card: Option<&IMFDXGIDeviceManager>) -> Result<(IMFSourceReader, u32, u32)> {
    // SAFETY: plain Media Foundation calls on valid objects.
    unsafe {
        let attrs = attributes(5)?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
        if let Some(m) = card {
            attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, m)?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            // The device stays open if this reader is given up for a
            // software one (it's shut down by hand at the end).
            attrs.SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1)?;
        }
        let reader = MFCreateSourceReaderFromMediaSource(source, &attrs).map_err(|e| explain(&e, &format!("Opening {name}")))?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
        if reader.SetCurrentMediaType(stream, None, &out).is_err() {
            // Not scaled by Windows: Cutaway scales instead.
            out.DeleteItem(&MF_MT_FRAME_SIZE)?;
            reader.SetCurrentMediaType(stream, None, &out).map_err(|e| explain(&e, &format!("{name} can't deliver pictures Cutaway can use")))?;
        }
        let current = reader.GetCurrentMediaType(stream)?;
        let (ow, oh) = unpack(current.GetUINT64(&MF_MT_FRAME_SIZE)?);
        Ok((reader, ow, oh))
    }
}

pub fn open_camera(info: &DeviceInfo, w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(Mta<IMFMediaSource>, String)>>();
    let (id, name, s2) = (info.id.clone(), info.name.clone(), stop.clone());
    let thread = std::thread::Builder::new().name(format!("camera {name}")).spawn(move || {
        let opened = (|| -> Result<(IMFMediaSource, IMFSourceReader, u32, u32, String, bool)> {
            mf()?;
            let act = activations()?
                .into_iter()
                .find(|a| string_attr(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).as_deref() == Some(id.as_str()))
                .ok_or_else(|| anyhow!("{name} is no longer connected"))?;
            // SAFETY: plain Media Foundation calls on valid objects.
            unsafe {
                let source: IMFMediaSource = act.ActivateObject().map_err(|e| explain(&e, &format!("Opening {name}")))?;
                // The device's own mode first…
                let (handler, list) = native_types(&source)?;
                let chosen = pick(&list, w, h, fps).map(|n| (n.ty.clone(), subtype_name(&n.subtype), n.width, n.height, n.fps));
                if let Some((ty, ..)) = &chosen {
                    if let Err(e) = handler.SetCurrentMediaType(ty) {
                        eprintln!("warning: {name}: couldn't set the capture mode ({e}); using its current one");
                    }
                }
                // …then NV12 at the source's size out of the reader (it
                // decodes MJPEG, converts and scales as needed). A camera
                // that sends compressed pictures has them decoded and
                // scaled on the graphics card when there is one.
                let packed = handler.GetCurrentMediaType().and_then(|t| t.GetGUID(&MF_MT_SUBTYPE)).is_ok_and(|g| compressed(&g));
                let card = if packed { super::dxgi_manager() } else { None };
                let (reader, ow, oh, card) = match card.as_ref().map(|m| open_reader(&source, &name, w, h, Some(m))) {
                    Some(Ok((r, ow, oh))) => (r, ow, oh, true),
                    Some(Err(e)) => {
                        eprintln!("{name}: the graphics card can't decode it ({e:#}); decoding in software");
                        let (r, ow, oh) = open_reader(&source, &name, w, h, None)?;
                        (r, ow, oh, false)
                    }
                    None => {
                        let (r, ow, oh) = open_reader(&source, &name, w, h, None)?;
                        (r, ow, oh, false)
                    }
                };
                let desc = match chosen {
                    Some((_, fmt, cw, ch, cf)) => format!("{name} · {cw}×{ch} {fmt} @ {cf:.0} fps"),
                    None => name.clone(),
                };
                Ok((source, reader, ow, oh, desc, card))
            }
        })();
        let (source, reader, mut ow, mut oh, desc, mut card) = match opened {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok((Mta(source.clone()), desc)));
        let mut reader = Some(reader);
        let mut times = TimeMap { offset: None };
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        // Pictures delivered, and ones the graphics card path couldn't give.
        let (mut delivered, mut unusable) = (0u64, 0u32);
        while !s2.load(Ordering::Relaxed) {
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            let Some(r) = &reader else { break };
            // SAFETY: valid out pointers; blocks until a frame (or shutdown).
            let read = unsafe { r.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) };
            let failed = read.is_err() || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0;
            if card && delivered == 0 && !s2.load(Ordering::Relaxed) && (failed || unusable >= 30) {
                // The graphics card couldn't do it after all: in software.
                eprintln!("{name}: the graphics card couldn't decode it; decoding in software");
                card = false;
                // (The old reader lets go of the device first.)
                drop(reader.take());
                match open_reader(&source, &name, w, h, None) {
                    Ok((r, w2, h2)) => (reader, ow, oh) = (Some(r), w2, h2),
                    Err(e) => {
                        eprintln!("{name}: {e:#}");
                        break;
                    }
                }
                continue;
            }
            if let Err(e) = read {
                if !s2.load(Ordering::Relaxed) {
                    eprintln!("{name}: {}", explain(&e, "capture stopped"));
                }
                break;
            }
            if flags & (MF_SOURCE_READERF_ERROR.0 as u32 | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            // SAFETY: reading an optional attribute.
            let device_time = unsafe { sample.GetUINT64(&MFSampleExtension_DeviceReferenceSystemTime) }.ok();
            let pts = times.host(ts, device_time);
            // (A picture on the graphics card is copied out by the lock.)
            match planes(&sample, ow, oh) {
                Some((y, uv)) => {
                    delivered += 1;
                    let frame = VideoFrame::new_nv12(ow & !1, oh & !1, pts, y, uv, 0);
                    sink(if (ow, oh) == (w, h) { frame } else { frame.scaled(w, h) });
                }
                None => unusable += 1,
            }
        }
        // SAFETY: plain call; the reader is done.
        let _ = unsafe { source.Shutdown() };
    })?;
    let (source, _desc) = rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| {
            // Don't let it start capturing later with nothing to stop it.
            stop.store(true, Ordering::Relaxed);
            anyhow!("{} didn't start", info.name)
        })?
        .context("camera")?;
    Ok(Box::new(Camera { stop, source, thread: Some(thread) }))
}

#[cfg(test)]
mod tests {
    use super::super::copy_nv12;
    use super::*;

    #[test]
    fn copies_padded_nv12() {
        // 4×2 picture, pitch 8, luma padded to 4 rows.
        let (w, h, pitch, padded) = (4usize, 2usize, 8usize, 4usize);
        let mut buf = vec![0u8; pitch * padded * 3 / 2];
        for r in 0..h {
            for c in 0..w {
                buf[r * pitch + c] = (r * 10 + c) as u8;
            }
        }
        for c in 0..w {
            buf[pitch * padded + c] = 100 + c as u8;
        }
        let (y, uv) = copy_nv12(buf.as_ptr(), pitch, buf.len(), w, h).unwrap();
        assert_eq!(y, vec![0, 1, 2, 3, 10, 11, 12, 13]);
        assert_eq!(uv, vec![100, 101, 102, 103]);
    }

    #[test]
    fn maps_timestamps() {
        let mut m = TimeMap { offset: None };
        let now = clock::now();
        // Already on the performance counter.
        let t = m.host(now / 100 - 200_000, None);
        assert!((now - t - 20_000_000).abs() < 5_000_000);
        // From zero: anchored to arrival, spacing kept.
        let a = m.host(0, None);
        let b = m.host(333_333, None);
        assert!(b - a <= 33_400_000 && a <= clock::now());
    }
}
