//! A display, a window or an app's window through Windows Graphics Capture
//! (without the yellow capture border on Windows 11), and the list of them,
//! with pictures, for Settings.
//!
//! The capture comes as BGRA textures; the graphics card's video processor
//! scales each to the source's size and converts it to NV12, its shape kept
//! (black around a window that's another shape), and only that comes back to
//! the CPU. If the card can't (or CUTAWAY_CPU_SCREEN_CONVERT=1), the BGRA
//! picture is copied back and converted on the CPU instead, the same way.
//! Windows only sends a new picture when the screen changes, so the last
//! one is repeated in between (a still screen is still a live source). When a captured window closes, the source goes
//! without a picture and is opened again, finding the window if it's back.

use super::com;
use crate::engine::sources::Running;
use crate::media::{bgra_to_nv12, clock, readback_nv12, Ns, VideoFrame, NS_PER_SEC};
use crate::platform::FrameSink;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::mem::ManuallyDrop;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use crate::platform::capture::{self, CaptureTarget, ScreenTarget, TargetKind, Thumb};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HMODULE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

struct Screen {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A CPU-readable copy of a texture.
struct Staging {
    tex: ID3D11Texture2D,
    size: (u32, u32),
}

/// The graphics card's video processor, scaling a BGRA capture and
/// converting it to NV12 (BT.709, video range, like `bgra_to_nv12`) at the
/// output size. Made for one input texture size and format.
struct Gpu {
    key: ((u32, u32), DXGI_FORMAT),
    vctx: ID3D11VideoContext,
    vp: ID3D11VideoProcessor,
    /// Our copy of the capture (the pool's textures come and go).
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    /// CPU-readable copy of `output`: 1.5 bytes a pixel.
    readback: ID3D11Texture2D,
}

/// A plain 2D texture description.
fn tex_desc(size: (u32, u32), format: DXGI_FORMAT, usage: D3D11_USAGE, bind: D3D11_BIND_FLAG, cpu: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: size.0,
        Height: size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    }
}

fn texture(device: &ID3D11Device, d: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
    let mut t = None;
    // SAFETY: plain call with a valid description.
    unsafe { device.CreateTexture2D(d, None, Some(&mut t))? };
    t.ok_or_else(|| anyhow!("no texture"))
}

impl Gpu {
    /// A processor for `in_size` textures of `format`, to `frame` NV12.
    fn new(device: &ID3D11Device, context: &ID3D11DeviceContext, in_size: (u32, u32), format: DXGI_FORMAT, frame: (u32, u32), fps: u32) -> Result<Gpu> {
        // SAFETY: Direct3D calls on valid objects with valid descriptions.
        unsafe {
            let vdev: ID3D11VideoDevice = device.cast().context("no video device")?;
            let vctx: ID3D11VideoContext = context.cast().context("no video context")?;
            let rate = DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: in_size.0,
                InputHeight: in_size.1,
                OutputFrameRate: rate,
                OutputWidth: frame.0,
                OutputHeight: frame.1,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let en = vdev.CreateVideoProcessorEnumerator(&content).context("video processor")?;
            let can = |f: DXGI_FORMAT, what: D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT| en.CheckVideoProcessorFormat(f).is_ok_and(|s| s & what.0 as u32 != 0);
            if !can(format, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT) || !can(DXGI_FORMAT_NV12, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT) {
                return Err(anyhow!("the video processor can't convert this format to NV12"));
            }
            let vp = vdev.CreateVideoProcessor(&en, 0).context("video processor")?;
            let input = texture(device, &tex_desc(in_size, format, D3D11_USAGE_DEFAULT, D3D11_BIND_FLAG(D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0), 0))?;
            let output = texture(device, &tex_desc(frame, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET, 0))?;
            let readback = texture(device, &tex_desc(frame, DXGI_FORMAT_NV12, D3D11_USAGE_STAGING, D3D11_BIND_FLAG(0), D3D11_CPU_ACCESS_READ.0 as u32))?;
            let ivd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
            };
            let mut input_view = None;
            vdev.CreateVideoProcessorInputView(&input, &en, &ivd, Some(&mut input_view)).context("video processor input")?;
            let ovd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output_view = None;
            vdev.CreateVideoProcessorOutputView(&output, &en, &ovd, Some(&mut output_view)).context("video processor output")?;
            // Colours as bgra_to_nv12: full-range RGB in, BT.709 video range out.
            if let Ok(vctx1) = vctx.cast::<ID3D11VideoContext1>() {
                vctx1.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                vctx1.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            } else {
                let cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: capture::VP_COLOR_SPACE_709_VIDEO };
                vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &cs);
                vctx.VideoProcessorSetOutputColorSpace(&vp, &cs);
            }
            vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // No driver "enhancements": the picture as it is.
            vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
            // Black around a picture of another shape (converted to 16/128/128).
            let black = D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } } };
            vctx.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
            let full = RECT { left: 0, top: 0, right: frame.0 as i32, bottom: frame.1 as i32 };
            vctx.VideoProcessorSetOutputTargetRect(&vp, true, Some(&full));
            Ok(Gpu {
                key: (in_size, format),
                vctx,
                vp,
                input,
                input_view: input_view.ok_or_else(|| anyhow!("no video processor input"))?,
                output,
                output_view: output_view.ok_or_else(|| anyhow!("no video processor output"))?,
                readback,
            })
        }
    }

    /// `tex` (the capture) scaled and converted into an NV12 frame, placed
    /// as `layout` says.
    fn convert(&self, context: &ID3D11DeviceContext, tex: &ID3D11Texture2D, layout: &capture::ScreenLayout, pts: Ns) -> Result<VideoFrame> {
        let (sw, sh) = layout.src;
        let (x, y, pw, ph) = layout.place;
        let (fw, fh) = layout.frame;
        // SAFETY: Direct3D calls on valid objects; the stream's view is a
        // counted reference released after the call; the readback copy is
        // mapped only while read.
        unsafe {
            context.CopyResource(&self.input, tex);
            let src = RECT { left: 0, top: 0, right: sw as i32, bottom: sh as i32 };
            let dst = RECT { left: x as i32, top: y as i32, right: (x + pw) as i32, bottom: (y + ph) as i32 };
            self.vctx.VideoProcessorSetStreamSourceRect(&self.vp, 0, true, Some(&src));
            self.vctx.VideoProcessorSetStreamDestRect(&self.vp, 0, true, Some(&dst));
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM { Enable: true.into(), pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())), ..Default::default() };
            let blt = self.vctx.VideoProcessorBlt(&self.vp, &self.output_view, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            blt.context("video processor")?;
            context.CopyResource(&self.readback, &self.output);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.readback, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let len = pitch * (fh as usize * 3 / 2);
            let frame = readback_nv12(std::slice::from_raw_parts(mapped.pData as *const u8, len), pitch, fw as usize, fh as usize, pts);
            context.Unmap(&self.readback, 0);
            frame.ok_or_else(|| anyhow!("the converted picture came back short"))
        }
    }
}

/// Said once a run: the graphics card can't convert screen pictures.
static GPU_FAILED_SAID: AtomicBool = AtomicBool::new(false);

/// What a target captures: a display (HMONITOR) or a window.
#[derive(Clone, Copy)]
enum Item {
    Monitor(HMONITOR),
    Window(HWND),
}

// SAFETY: plain handles, used from the capture thread.
unsafe impl Send for Item {}

fn find_item(target: &ScreenTarget) -> Result<Item> {
    match target {
        ScreenTarget::Main | ScreenTarget::Display(_) => {
            let list = crate::platform::displays::list();
            let name = if let ScreenTarget::Display(n) = target { Some(n.as_str()) } else { None };
            let d = name.and_then(|n| list.iter().find(|d| d.name == n)).or_else(|| list.iter().find(|d| d.main));
            Ok(Item::Monitor(match d {
                Some(d) => HMONITOR(d.id as *mut _),
                // SAFETY: plain call.
                None => unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) },
            }))
        }
        ScreenTarget::Window { app, title } => {
            let windows: Vec<(HWND, String)> = top_windows().into_iter().filter(|w| w.exe.eq_ignore_ascii_case(app)).map(|w| (w.hwnd, w.title)).collect();
            capture::best_window(&windows, title).map(Item::Window).ok_or_else(|| anyhow!("{} has no window open", capture::short_app(app)))
        }
        ScreenTarget::App(app) => top_windows()
            .into_iter()
            .filter(|w| w.exe.eq_ignore_ascii_case(app))
            .max_by_key(|w| w.size.0 as i64 * w.size.1 as i64)
            .map(|w| Item::Window(w.hwnd))
            .ok_or_else(|| anyhow!("{} isn't open (or has no windows)", capture::short_app(app))),
    }
}

pub fn open_screen(target: &ScreenTarget, w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
    let item = find_item(target)?;
    let want_gpu = !capture::cpu_screen_convert_forced(std::env::var_os("CUTAWAY_CPU_SCREEN_CONVERT").as_deref());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<()>>();
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("screen capture".into()).spawn(move || {
        com();
        // SAFETY: plain Direct3D and WinRT calls on objects made here.
        let setup = (|| -> Result<_> {
            unsafe {
                let (mut device, mut context) = (None, None);
                let create = |flags, device: &mut _, context: &mut _| D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(device), None, Some(context));
                // Video support for the GPU conversion; without it if the
                // card won't (the CPU conversion is used then).
                let video = if want_gpu { create(D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT, &mut device, &mut context).is_ok() } else { false };
                if !video {
                    create(D3D11_CREATE_DEVICE_BGRA_SUPPORT, &mut device, &mut context).context("Direct3D")?;
                }
                let (device, context) = (device.ok_or_else(|| anyhow!("no Direct3D device"))?, context.ok_or_else(|| anyhow!("no Direct3D context"))?);
                let winrt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()?;
                let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
                let item: GraphicsCaptureItem = match item {
                    Item::Monitor(m) => interop.CreateForMonitor(m).context("screen capture isn't available")?,
                    Item::Window(hwnd) => interop.CreateForWindow(hwnd).context("that window can't be captured")?,
                };
                let size = item.Size()?;
                let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
                let session = pool.CreateCaptureSession(&item)?;
                let _ = session.SetIsCursorCaptureEnabled(true);
                // Windows 11: no yellow border around the captured screen.
                let _ = session.SetIsBorderRequired(false);
                session.StartCapture()?;
                Ok((device, context, winrt, pool, session, size))
            }
        })();
        let (device, context, winrt, pool, session, mut size) = match setup {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let _ = tx.send(Ok(()));
        let mut staging: Option<Staging> = None;
        // The GPU conversion, made for the capture's size when first needed;
        // `gpu_ok` turns false for good if any step of it fails.
        let mut gpu: Option<Gpu> = None;
        let mut gpu_ok = want_gpu;
        let mut last: Option<VideoFrame> = None;
        let period = NS_PER_SEC / fps.max(1) as i64;
        let mut next = clock::now();
        let mut checked = clock::now();
        while !s2.load(Ordering::Relaxed) {
            // A window that closed: stop, so the source shows no signal and
            // is opened again (finding the window if it comes back).
            if let Item::Window(hwnd) = item {
                if clock::now() - checked > NS_PER_SEC / 2 {
                    checked = clock::now();
                    // SAFETY: plain call; any handle value is allowed.
                    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                        break;
                    }
                }
            }
            next += period;
            clock::sleep_until(next);
            if clock::now() - next > NS_PER_SEC / 4 {
                next = clock::now(); // fell behind: don't try to catch up
            }
            // The newest picture, if the screen changed.
            let mut newest = None;
            while let Ok(f) = pool.TryGetNextFrame() {
                newest = Some(f);
            }
            if let Some(frame) = newest {
                let pts = frame.SystemRelativeTime().map(|t| clock::from_100ns(t.Duration)).unwrap_or_else(|_| clock::now());
                let pts = if (pts - clock::now()).abs() < NS_PER_SEC { pts } else { clock::now() };
                // The screen changed size: capture at the new size.
                if let Ok(cs) = frame.ContentSize() {
                    if (cs.Width, cs.Height) != (size.Width, size.Height) && cs.Width > 0 && cs.Height > 0 {
                        size = cs;
                        let _ = pool.Recreate(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size);
                    }
                }
                // SAFETY: Direct3D calls on valid objects; the staging copy
                // is mapped only while read.
                let picture = unsafe {
                    (|| -> Result<VideoFrame> {
                        let surface = frame.Surface()?;
                        let tex: ID3D11Texture2D = surface.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
                        let mut desc = D3D11_TEXTURE2D_DESC::default();
                        tex.GetDesc(&mut desc);
                        // The picture's shape kept: a window (or a display
                        // of another shape) sits on black.
                        let layout = capture::screen_layout((size.Width.max(0) as u32, size.Height.max(0) as u32), (desc.Width, desc.Height), (w, h));
                        if gpu_ok {
                            let key = ((desc.Width, desc.Height), desc.Format);
                            let converted = (|| {
                                if gpu.as_ref().map(|g| g.key) != Some(key) {
                                    gpu = None;
                                    gpu = Some(Gpu::new(&device, &context, key.0, key.1, layout.frame, fps)?);
                                }
                                gpu.as_ref().unwrap().convert(&context, &tex, &layout, pts)
                            })();
                            match converted {
                                Ok(f) => return Ok(f),
                                // From now on on the CPU (this picture too).
                                Err(e) => {
                                    (gpu_ok, gpu) = (false, None);
                                    if !GPU_FAILED_SAID.swap(true, Ordering::Relaxed) {
                                        eprintln!("screen capture: the graphics card can't convert the picture ({e:#}); converting on the CPU");
                                    }
                                }
                            }
                        }
                        if staging.as_ref().map(|s| s.size) != Some((desc.Width, desc.Height)) {
                            let sd = tex_desc((desc.Width, desc.Height), desc.Format, D3D11_USAGE_STAGING, D3D11_BIND_FLAG(0), D3D11_CPU_ACCESS_READ.0 as u32);
                            staging = Some(Staging { tex: texture(&device, &sd)?, size: (desc.Width, desc.Height) });
                        }
                        let st = staging.as_ref().unwrap();
                        context.CopyResource(&st.tex, &tex);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context.Map(&st.tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                        let (sh, pitch) = (desc.Height as usize, mapped.RowPitch as usize);
                        let bytes = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * sh);
                        let (cw, ch) = (layout.src.0 as usize, layout.src.1 as usize);
                        let (fw, fh) = (layout.frame.0 as usize, layout.frame.1 as usize);
                        let (x, yo, pw, ph) = layout.place;
                        let (y, uv) = if layout.fills() {
                            bgra_to_nv12(bytes, pitch, cw, ch, fw, fh)
                        } else {
                            let (py, puv) = bgra_to_nv12(bytes, pitch, cw, ch, pw as usize, ph as usize);
                            capture::place_nv12(&py, &puv, pw as usize, ph as usize, fw, fh, x as usize, yo as usize)
                        };
                        context.Unmap(&st.tex, 0);
                        Ok(VideoFrame::new_nv12(fw as u32, fh as u32, pts, y, uv, 0))
                    })()
                };
                match picture {
                    Ok(f) => {
                        last = Some(f.clone());
                        sink(f);
                    }
                    Err(e) => eprintln!("screen capture: {e:#}"),
                }
            } else if let Some(f) = &last {
                // Still screen: the same picture again, now.
                let mut again = f.clone();
                again.pts = clock::now();
                again.serial = VideoFrame::REPEAT;
                sink(again);
            }
        }
        let _ = session.Close();
        let _ = pool.Close();
    })?;
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| {
        // Don't let it start capturing later with nothing to stop it.
        stop.store(true, Ordering::Relaxed);
        anyhow!("screen capture didn't start")
    })??;
    Ok(Box::new(Screen { stop, thread: Some(thread) }))
}

/// A top-level window worth offering.
struct TopWindow {
    hwnd: HWND,
    title: String,
    /// Its program's file name ("chrome.exe").
    exe: String,
    size: (i32, i32),
}

/// Windows as Alt+Tab shows them, frontmost first: visible, not minimised,
/// titled, not tool windows or hidden ("cloaked") ones, and not Cutaway's.
fn top_windows() -> Vec<TopWindow> {
    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
        // SAFETY: `out` is the Vec below, alive for the call.
        unsafe { (*(out.0 as *mut Vec<HWND>)).push(hwnd) };
        true.into()
    }
    let mut all: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes onto the Vec, during this call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut all as *mut _ as isize)) };
    // SAFETY: plain calls on window handles that may have gone (they fail).
    let me = unsafe { GetCurrentProcessId() };
    all.into_iter()
        .filter_map(|hwnd| unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                return None;
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
                return None;
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if title.trim().is_empty() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == me {
                return None;
            }
            let r = window_rect(hwnd)?;
            let size = (r.right - r.left, r.bottom - r.top);
            if size.0 < 60 || size.1 < 40 {
                return None;
            }
            Some(TopWindow { hwnd, title, exe: exe_name(pid)?, size })
        })
        .collect()
}

/// A window's rectangle on screen, without its invisible resize border.
fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: plain calls into a RECT.
    unsafe {
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
            GetWindowRect(hwnd, &mut r).ok()?;
        }
    }
    Some(r)
}

/// A process's program file name ("POWERPNT.EXE").
fn exe_name(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed below; the buffer is sized in `len`.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(|s| s.to_string())
    }
}

/// Thumbnails' longest side, in pixels.
const THUMB: usize = 320;

/// Draw into a 32-bit bitmap `w`×`h` with `paint`, and shrink it to a
/// thumbnail.
fn grab(w: i32, h: i32, paint: impl FnOnce(HDC) -> bool) -> Option<Thumb> {
    if w <= 0 || h <= 0 {
        return None;
    }
    // SAFETY: GDI objects made here and released below; the DIB's bits
    // are read only while it exists.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let mut out = None;
        if let Some(bmp) = bmp {
            let old = SelectObject(dc, bmp.into());
            if paint(dc) && !bits.is_null() {
                let px = std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4);
                let mut t = capture::shrink_rgba(px, w as usize, h as usize, w as usize * 4, THUMB);
                capture::bgra_to_rgba(&mut t);
                out = Some(t);
            }
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        out
    }
}

/// A display's picture, from the screen.
fn display_thumb(d: &crate::platform::displays::Display) -> Option<Thumb> {
    let (x, y, w, h) = (d.x as i32, d.y as i32, d.w as i32, d.h as i32);
    grab(w, h, |dc| unsafe {
        // SAFETY: copying from the whole-screen DC into ours.
        let screen = GetDC(None);
        let ok = BitBlt(dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY).is_ok();
        ReleaseDC(None, screen);
        ok
    })
}

/// A window's picture, drawn by the window itself (so it works when it's
/// covered by others).
fn window_thumb(hwnd: HWND) -> Option<Thumb> {
    let r = window_rect(hwnd)?;
    let mut wr = RECT::default();
    // SAFETY: plain call into a RECT.
    unsafe { GetWindowRect(hwnd, &mut wr).ok()? };
    let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
    // PW_RENDERFULLCONTENT: windows drawn by the GPU too (browsers, apps).
    let t = grab(w, h, |dc| unsafe { PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(2)).as_bool() })?;
    // Trim the invisible border PrintWindow includes.
    let k = t.size[0] as f32 / w.max(1) as f32;
    let (l, tp) = (((r.left - wr.left) as f32 * k) as usize, ((r.top - wr.top) as f32 * k) as usize);
    let (cw, ch) = ((((r.right - r.left) as f32) * k) as usize, (((r.bottom - r.top) as f32) * k) as usize);
    let (cw, ch) = (cw.min(t.size[0].saturating_sub(l)).max(1), ch.min(t.size[1].saturating_sub(tp)).max(1));
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for row in tp..tp + ch {
        rgba.extend_from_slice(&t.rgba[(row * t.size[0] + l) * 4..][..cw * 4]);
    }
    Some(Thumb { size: [cw, ch], rgba })
}

/// Every display, window and app that can be captured, with pictures.
/// Slow (a second or two): call it off the UI thread.
pub fn capture_targets() -> Result<Vec<CaptureTarget>> {
    let mut out = Vec::new();
    for d in crate::platform::displays::list() {
        out.push(CaptureTarget {
            kind: TargetKind::Display,
            device: if d.main { "screen".into() } else { ScreenTarget::Display(d.name.clone()).device() },
            name: d.name.clone(),
            detail: format!("{}×{}", d.pixels.0, d.pixels.1),
            thumb: display_thumb(&d),
        });
    }
    let mut apps: Vec<(String, Option<Thumb>)> = Vec::new();
    for w in top_windows().into_iter().take(40) {
        let thumb = window_thumb(w.hwnd);
        if !apps.iter().any(|a| a.0.eq_ignore_ascii_case(&w.exe)) {
            apps.push((w.exe.clone(), thumb.clone()));
        }
        out.push(CaptureTarget {
            kind: TargetKind::Window,
            device: ScreenTarget::Window { app: w.exe.clone(), title: w.title.clone() }.device(),
            name: w.title,
            detail: capture::short_app(&w.exe),
            thumb,
        });
    }
    apps.sort_by_key(|a| a.0.to_lowercase());
    for (exe, thumb) in apps {
        out.push(CaptureTarget { kind: TargetKind::App, device: ScreenTarget::App(exe.clone()).device(), name: capture::short_app(&exe), detail: "every window of the app (the biggest)".into(), thumb });
    }
    Ok(out)
}
