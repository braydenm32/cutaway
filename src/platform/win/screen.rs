//! The main screen through Windows Graphics Capture (without the yellow
//! capture border on Windows 11).
//!
//! The capture comes as BGRA textures; each is copied to the CPU and
//! converted to NV12 at the source's size. Windows only sends a new
//! picture when the screen changes, so the last one is repeated in between
//! (a still screen is still a live source).

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
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
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

pub fn open_screen(w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
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
                let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
                let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
                let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor).context("screen capture isn't available")?;
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
        while !s2.load(Ordering::Relaxed) {
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
                        let (y, uv) = bgra_to_nv12(bytes, pitch, sw, sh, w as usize, h as usize);
                        context.Unmap(&st.tex, 0);
                        Ok(VideoFrame::new_nv12(w & !1, h & !1, pts, y, uv, 0))
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
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("screen capture didn't start"))??;
    Ok(Box::new(Screen { stop, thread: Some(thread) }))
}
