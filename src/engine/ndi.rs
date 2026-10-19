//! NDI input. NDI's licence doesn't allow bundling its library, so this
//! uses the free NDI runtime (installed with NDI Tools) when it's present
//! and explains how to get it when it isn't.

use super::feed::{AudioHub, Feeder};
use super::netsrc::{NetState, NetStatus};
use super::sources::{Running, SourceSlot};
use crate::config::SourceConfig;
use crate::media::{clock, VideoFrame};
use anyhow::{anyhow, bail, Result};
use libloading::Library;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    name: *const c_char,
    url: *const c_char,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    groups: *const c_char,
    extra_ips: *const c_char,
}

#[repr(C)]
struct RecvCreate {
    source: Source,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    name: *const c_char,
}

#[repr(C)]
struct VideoFrameV2 {
    xres: i32,
    yres: i32,
    fourcc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    aspect: f32,
    format_type: i32,
    timecode: i64,
    data: *mut u8,
    stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

#[repr(C)]
struct AudioFrameV2 {
    sample_rate: i32,
    channels: i32,
    samples: i32,
    timecode: i64,
    data: *mut f32,
    channel_stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

const FRAME_VIDEO: i32 = 1;
const FRAME_AUDIO: i32 = 2;
const FRAME_ERROR: i32 = 4;
const COLOR_UYVY_BGRA: i32 = 1;
const BANDWIDTH_HIGHEST: i32 = 100;
const TIMESTAMP_UNDEFINED: i64 = i64::MAX;

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

struct Api {
    _lib: Library,
    find_create: unsafe extern "C" fn(*const FindCreate) -> *mut c_void,
    find_wait: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    find_sources: unsafe extern "C" fn(*mut c_void, *mut u32) -> *const Source,
    find_destroy: unsafe extern "C" fn(*mut c_void),
    recv_create: unsafe extern "C" fn(*const RecvCreate) -> *mut c_void,
    recv_destroy: unsafe extern "C" fn(*mut c_void),
    recv_capture: unsafe extern "C" fn(*mut c_void, *mut VideoFrameV2, *mut AudioFrameV2, *mut c_void, u32) -> i32,
    free_video: unsafe extern "C" fn(*mut c_void, *const VideoFrameV2),
    free_audio: unsafe extern "C" fn(*mut c_void, *const AudioFrameV2),
    ptz: Option<PtzApi>,
}

/// NDI's PTZ controls (in every runtime since NDI 3).
#[derive(Clone, Copy)]
struct PtzApi {
    supported: unsafe extern "C" fn(*mut c_void) -> bool,
    pan_tilt_speed: unsafe extern "C" fn(*mut c_void, f32, f32) -> bool,
    zoom_speed: unsafe extern "C" fn(*mut c_void, f32) -> bool,
    store: unsafe extern "C" fn(*mut c_void, i32) -> bool,
    recall: unsafe extern "C" fn(*mut c_void, i32, f32) -> bool,
    pan_tilt: unsafe extern "C" fn(*mut c_void, f32, f32) -> bool,
    zoom: unsafe extern "C" fn(*mut c_void, f32) -> bool,
}

// Function pointers into a library that stays loaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn candidates() -> Vec<String> {
    let mut v = Vec::new();
    for var in ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
        if let Ok(dir) = std::env::var(var) {
            for name in ["libndi.dylib", "libndi.so", "Processing.NDI.Lib.x64.dll"] {
                v.push(format!("{dir}/{name}"));
            }
        }
    }
    if cfg!(target_os = "macos") {
        v.extend(
            ["/usr/local/lib/libndi.dylib", "/Library/NDI SDK for Apple/lib/macOS/libndi.dylib", "/Library/Application Support/NewTek/NDI/libndi.dylib", "/Applications/NDI Video Monitor.app/Contents/Frameworks/libndi.dylib"]
                .map(String::from),
        );
    } else if cfg!(windows) {
        v.push("Processing.NDI.Lib.x64.dll".into());
    } else {
        v.extend(["libndi.so.6", "libndi.so.5", "libndi.so"].map(String::from));
    }
    v
}

fn load() -> Result<Api, String> {
    for path in candidates() {
        let Ok(lib) = (unsafe { Library::new(&path) }) else { continue };
        let r = (|| -> Result<Api, libloading::Error> {
            unsafe {
                let init: libloading::Symbol<unsafe extern "C" fn() -> bool> = lib.get(b"NDIlib_initialize\0")?;
                if !init() {
                    return Err(libloading::Error::DlSymUnknown);
                }
                Ok(Api {
                    find_create: *lib.get(b"NDIlib_find_create_v2\0")?,
                    find_wait: *lib.get(b"NDIlib_find_wait_for_sources\0")?,
                    find_sources: *lib.get(b"NDIlib_find_get_current_sources\0")?,
                    find_destroy: *lib.get(b"NDIlib_find_destroy\0")?,
                    recv_create: *lib.get(b"NDIlib_recv_create_v3\0")?,
                    recv_destroy: *lib.get(b"NDIlib_recv_destroy\0")?,
                    recv_capture: *lib.get(b"NDIlib_recv_capture_v2\0")?,
                    free_video: *lib.get(b"NDIlib_recv_free_video_v2\0")?,
                    free_audio: *lib.get(b"NDIlib_recv_free_audio_v2\0")?,
                    ptz: (|| -> Result<PtzApi, libloading::Error> {
                        Ok(PtzApi {
                            supported: *lib.get(b"NDIlib_recv_ptz_is_supported\0")?,
                            pan_tilt_speed: *lib.get(b"NDIlib_recv_ptz_pan_tilt_speed\0")?,
                            zoom_speed: *lib.get(b"NDIlib_recv_ptz_zoom_speed\0")?,
                            store: *lib.get(b"NDIlib_recv_ptz_store_preset\0")?,
                            recall: *lib.get(b"NDIlib_recv_ptz_recall_preset\0")?,
                            pan_tilt: *lib.get(b"NDIlib_recv_ptz_pan_tilt\0")?,
                            zoom: *lib.get(b"NDIlib_recv_ptz_zoom\0")?,
                        })
                    })()
                    .ok(),
                    _lib: lib,
                })
            }
        })();
        if let Ok(api) = r {
            return Ok(api);
        }
    }
    Err("NDI isn't installed on this computer. Install the free NDI Tools from ndi.video/tools, then restart Cutaway.".into())
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(|e| anyhow!("{e}"))
}

pub fn available() -> bool {
    api().is_ok()
}

/// NDI sources on the network (waits up to `ms` for them to show up).
pub fn find_sources(ms: u32) -> Result<Vec<String>> {
    let api = api()?;
    unsafe {
        let settings = FindCreate { show_local_sources: true, groups: std::ptr::null(), extra_ips: std::ptr::null() };
        let f = (api.find_create)(&settings);
        if f.is_null() {
            bail!("NDI couldn't start looking for sources");
        }
        (api.find_wait)(f, ms);
        let mut n = 0u32;
        let list = (api.find_sources)(f, &mut n);
        let mut out = Vec::new();
        for i in 0..n as usize {
            let s = *list.add(i);
            if !s.name.is_null() {
                out.push(CStr::from_ptr(s.name).to_string_lossy().to_string());
            }
        }
        (api.find_destroy)(f);
        Ok(out)
    }
}

/// UYVY (4:2:2) to NV12.
fn uyvy_to_nv12(data: &[u8], stride: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (w & !1, h & !1);
    let mut y = vec![0u8; w * h];
    let mut uv = vec![0u8; w * h / 2];
    for r in 0..h {
        let row = &data[r * stride..r * stride + w * 2];
        for x in 0..w / 2 {
            y[r * w + x * 2] = row[x * 4 + 1];
            y[r * w + x * 2 + 1] = row[x * 4 + 3];
        }
        if r % 2 == 0 && r + 1 < h {
            let next = &data[(r + 1) * stride..(r + 1) * stride + w * 2];
            for x in 0..w / 2 {
                uv[(r / 2) * w + x * 2] = ((row[x * 4] as u16 + next[x * 4] as u16) / 2) as u8;
                uv[(r / 2) * w + x * 2 + 1] = ((row[x * 4 + 2] as u16 + next[x * 4 + 2] as u16) / 2) as u8;
            }
        }
    }
    (y, uv)
}

/// BGRA/BGRX/RGBA/RGBX to NV12.
fn rgb_to_nv12(data: &[u8], stride: usize, w: usize, h: usize, bgr: bool) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (w & !1, h & !1);
    let mut y = vec![0u8; w * h];
    let mut uv = vec![0u8; w * h / 2];
    let px = |x: usize, r: usize| {
        let p = &data[r * stride + x * 4..r * stride + x * 4 + 3];
        let (rr, g, b) = if bgr { (p[2], p[1], p[0]) } else { (p[0], p[1], p[2]) };
        (rr as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    };
    for r in 0..h {
        for x in 0..w {
            let (rr, g, b) = px(x, r);
            y[r * w + x] = (16.0 + 219.0 * (0.2126 * rr + 0.7152 * g + 0.0722 * b)) as u8;
        }
    }
    for r in 0..h / 2 {
        for x in 0..w / 2 {
            let (rr, g, b) = px(x * 2, r * 2);
            uv[r * w + x * 2] = (128.0 + 224.0 * (-0.1146 * rr - 0.3854 * g + 0.5 * b)) as u8;
            uv[r * w + x * 2 + 1] = (128.0 + 224.0 * (0.5 * rr - 0.4542 * g - 0.0458 * b)) as u8;
        }
    }
    (y, uv)
}

struct NdiRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for NdiRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Pass PTZ moves to an NDI camera.
unsafe fn ptz_move(p: &PtzApi, recv: *mut c_void, m: super::ptz::Move) {
    use super::ptz::Move;
    match m {
        Move::Drive { pan, tilt } => {
            (p.pan_tilt_speed)(recv, pan, tilt);
        }
        Move::Zoom(z) => {
            (p.zoom_speed)(recv, z);
        }
        Move::Stop => {
            (p.pan_tilt_speed)(recv, 0.0, 0.0);
            (p.zoom_speed)(recv, 0.0);
        }
        Move::Home => {
            (p.pan_tilt)(recv, 0.0, 0.0);
            (p.zoom)(recv, 0.0);
        }
        Move::Recall(n) => {
            (p.recall)(recv, n.min(99) as i32, 1.0);
        }
        Move::Store(n) => {
            (p.store)(recv, n.min(99) as i32);
        }
    }
}

pub fn start(
    source: &str,
    cfg: &SourceConfig,
    slot: Arc<SourceSlot>,
    hub: Arc<AudioHub>,
    state: NetStatus,
    ptz_rx: Option<crossbeam_channel::Receiver<super::ptz::Move>>,
) -> Result<(Box<dyn Running>, String)> {
    let api = api()?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut feeder = Feeder::new(&cfg.name, slot, hub, (cfg.width, cfg.height), cfg.network_latency_ms.min(1000), stop.clone());
    let wanted = source.to_string();
    let recv_name = CString::new(format!("Cutaway {}", cfg.name)).unwrap_or_default();
    *state.lock().unwrap() = NetState { connected: false, detail: "looking for the NDI source".into() };
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name(format!("ndi {}", cfg.name)).spawn(move || unsafe {
        let name = CString::new(wanted.clone()).unwrap_or_default();
        let settings = RecvCreate {
            source: Source { name: name.as_ptr(), url: std::ptr::null() },
            color_format: COLOR_UYVY_BGRA,
            bandwidth: BANDWIDTH_HIGHEST,
            allow_video_fields: false,
            name: recv_name.as_ptr(),
        };
        let recv = (api.recv_create)(&settings);
        if recv.is_null() {
            state.lock().unwrap().detail = "NDI couldn't create a receiver".into();
            return;
        }
        let mut mix = None;
        let mut last_frame = std::time::Instant::now();
        while !s2.load(Ordering::Relaxed) {
            // PTZ moves for this camera.
            if let (Some(rx), Some(p)) = (&ptz_rx, &api.ptz) {
                while let Ok(m) = rx.try_recv() {
                    if (p.supported)(recv) {
                        ptz_move(p, recv, m);
                    }
                }
            }
            let mut v: VideoFrameV2 = std::mem::zeroed();
            let mut a: AudioFrameV2 = std::mem::zeroed();
            match (api.recv_capture)(recv, &mut v, &mut a, std::ptr::null_mut(), 200) {
                FRAME_VIDEO => {
                    last_frame = std::time::Instant::now();
                    let (w, h, stride) = (v.xres.max(2) as usize, v.yres.max(2) as usize, v.stride.max(0) as usize);
                    let len = stride * h;
                    if !v.data.is_null() && stride > 0 {
                        let data = std::slice::from_raw_parts(v.data, len);
                        let planes = if v.fourcc == fourcc(b"UYVY") {
                            Some(uyvy_to_nv12(data, stride, w, h))
                        } else if v.fourcc == fourcc(b"BGRA") || v.fourcc == fourcc(b"BGRX") {
                            Some(rgb_to_nv12(data, stride, w, h, true))
                        } else if v.fourcc == fourcc(b"RGBA") || v.fourcc == fourcc(b"RGBX") {
                            Some(rgb_to_nv12(data, stride, w, h, false))
                        } else {
                            None
                        };
                        if let Some((y, uv)) = planes {
                            let t = if v.timestamp == TIMESTAMP_UNDEFINED || v.timestamp == 0 { clock::now() } else { v.timestamp * 100 };
                            feeder.picture(t, VideoFrame::new_nv12((w & !1) as u32, (h & !1) as u32, 0, y, uv, 0));
                        }
                    }
                    (api.free_video)(recv, &v);
                    let mut st = state.lock().unwrap();
                    if !st.connected {
                        st.connected = true;
                        st.detail = format!("receiving {}×{}", v.xres, v.yres);
                    }
                }
                FRAME_AUDIO => {
                    let (ch, n, stride) = (a.channels.max(1) as usize, a.samples.max(0) as usize, (a.channel_stride / 4).max(0) as usize);
                    if !a.data.is_null() && n > 0 {
                        let planar = std::slice::from_raw_parts(a.data, stride * ch);
                        let mut inter = Vec::with_capacity(n * ch);
                        for i in 0..n {
                            for c in 0..ch {
                                inter.push(planar[c * stride + i]);
                            }
                        }
                        let t = if a.timestamp == TIMESTAMP_UNDEFINED || a.timestamp == 0 { clock::now() } else { a.timestamp * 100 };
                        feeder.pcm(t, a.sample_rate.max(1) as u32, ch as u16, &inter, &mut mix);
                    }
                    (api.free_audio)(recv, &a);
                }
                FRAME_ERROR => {
                    feeder.reset();
                    *state.lock().unwrap() = NetState { connected: false, detail: "connection lost; waiting for the NDI source".into() };
                }
                _ => {
                    if last_frame.elapsed().as_secs() >= 3 {
                        let mut st = state.lock().unwrap();
                        if st.connected {
                            st.connected = false;
                            st.detail = "waiting for the NDI source".into();
                            feeder.reset();
                        }
                    }
                }
            }
        }
        (api.recv_destroy)(recv);
    })?;
    Ok((Box::new(NdiRunning { stop, thread: Some(thread) }), format!("NDI: {source}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_uyvy() {
        // 2×2 UYVY: white-ish top row, black bottom row.
        let data = [128u8, 235, 128, 235, 128, 16, 128, 16];
        let (y, uv) = uyvy_to_nv12(&data, 4, 2, 2);
        assert_eq!(y, vec![235, 235, 16, 16]);
        assert_eq!(uv, vec![128, 128]);
        // Without the runtime installed this reports how to get it.
        if !available() {
            assert!(find_sources(10).unwrap_err().to_string().contains("NDI Tools"));
        }
    }
}
