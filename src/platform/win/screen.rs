//! A display, a window or an app's window through Windows Graphics Capture
//! (without the yellow capture border on Windows 11), and the list of them,
//! with pictures, for Settings.
//!
//! The capture comes as BGRA textures; each is copied to the CPU and
//! converted to NV12 at the source's size, its shape kept (black around a
//! window that's another shape). Windows only sends a new picture when the
//! screen changes, so the last one is repeated in between (a still screen
//! is still a live source). When a captured window closes, the source goes
//! without a picture and is opened again, finding the window if it's back.

use super::com;
use crate::engine::sources::Running;
use crate::media::{bgra_to_nv12, clock, VideoFrame, NS_PER_SEC};
use crate::platform::FrameSink;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
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
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<()>>();
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("screen capture".into()).spawn(move || {
        com();
        // SAFETY: plain Direct3D and WinRT calls on objects made here.
        let setup = (|| -> Result<_> {
            unsafe {
                let (mut device, mut context) = (None, None);
                D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))
                    .context("Direct3D")?;
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
                        if staging.as_ref().map(|s| s.size) != Some((desc.Width, desc.Height)) {
                            let sd = D3D11_TEXTURE2D_DESC {
                                Width: desc.Width,
                                Height: desc.Height,
                                MipLevels: 1,
                                ArraySize: 1,
                                Format: desc.Format,
                                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                                Usage: D3D11_USAGE_STAGING,
                                BindFlags: 0,
                                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                                MiscFlags: 0,
                            };
                            let mut t = None;
                            device.CreateTexture2D(&sd, None, Some(&mut t))?;
                            staging = Some(Staging { tex: t.ok_or_else(|| anyhow!("no staging texture"))?, size: (desc.Width, desc.Height) });
                        }
                        let st = staging.as_ref().unwrap();
                        context.CopyResource(&st.tex, &tex);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context.Map(&st.tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                        let (sw, sh, pitch) = (desc.Width as usize, desc.Height as usize, mapped.RowPitch as usize);
                        let bytes = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * sh);
                        // The picture's shape kept: a window (or a display
                        // of another shape) sits on black.
                        let (cw, ch) = (size.Width.max(1) as usize, size.Height.max(1) as usize);
                        let (cw, ch) = (cw.min(sw), ch.min(sh));
                        let (fw, fh) = (w as usize & !1, h as usize & !1);
                        let (x, yo, pw, ph) = capture::fit_inside(cw as u32, ch as u32, fw as u32, fh as u32);
                        let (y, uv) = if (pw as usize, ph as usize) == (fw, fh) {
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
