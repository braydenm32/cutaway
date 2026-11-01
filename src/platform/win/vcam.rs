//! The "Cutaway" camera on Windows 11: while the Webcam button is on,
//! Cutaway creates a virtual camera (MFCreateVirtualCamera) backed by its
//! camera source (`cutaway_vcam.dll`, loaded by Windows' camera service)
//! and hands it the program through shared memory.
//!
//! The camera source is installed once, as administrator: copied to
//! Program Files (where the camera service can read it) and registered.

#[path = "../../../windows/vcam/src/shared.rs"]
mod shared;

use super::com;
use crate::engine::sources::Running;
use crate::media::VideoFrame;
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA;
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session, MFVirtualCameraType_SoftwareCameraSource,
};
use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE};
use windows::Win32::System::SystemInformation::GetTickCount64;

const DLL: &str = "cutaway_vcam.dll";

/// Where the camera source is installed for the camera service.
fn installed_dll() -> PathBuf {
    let pf = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Program Files"));
    pf.join("Cutaway").join(DLL)
}

/// The camera source's file as registered, if it is.
fn registered_path() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let key = HSTRING::from(format!("SOFTWARE\\Classes\\CLSID\\{}\\InprocServer32", shared::CLSID_TEXT));
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: valid buffer and length.
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &key, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)) };
    if r.is_err() {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

pub fn installed() -> bool {
    registered_path().is_some_and(|p| p.exists())
}

/// Installed, but not the camera that came with this version of Cutaway
/// (after an update that changed it).
pub fn needs_reinstall() -> bool {
    let (Some(installed), Ok(ours)) = (registered_path(), source_dll()) else { return false };
    match (std::fs::read(&installed), std::fs::read(&ours)) {
        (Ok(a), Ok(b)) => a != b,
        _ => false,
    }
}

/// Install (or remove) the camera source. Asks Windows for administrator
/// rights (a UAC prompt) and does it in an elevated copy of Cutaway.
pub fn install(remove: bool) -> Result<String> {
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    if !remove && !source_dll()?.exists() {
        bail!("{DLL} isn't next to Cutaway (it comes with the Windows download)");
    }
    let exe = HSTRING::from(std::env::current_exe()?.as_os_str());
    let args = HSTRING::from(if remove { "--vcam-uninstall" } else { "--vcam-install" });
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: windows::core::w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: 0,
        ..Default::default()
    };
    // SAFETY: the strings live until the process has started.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|_| anyhow!("Windows didn't allow it (administrator rights are needed once)"))?;
    let mut code = 1u32;
    // SAFETY: the process handle is ours to wait on and close.
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    if code != 0 {
        bail!("the installer couldn't finish (see the log)");
    }
    Ok(if remove { "Removed the Cutaway camera".into() } else { "Installed: turn on Webcam, then choose \"Cutaway\" as the camera in your video app".into() })
}

fn source_dll() -> Result<PathBuf> {
    Ok(std::env::current_exe()?.parent().context("no folder")?.join(DLL))
}

/// The elevated part (`--vcam-install` / `--vcam-uninstall`).
pub fn install_elevated(remove: bool) -> Result<()> {
    let target = installed_dll();
    if remove {
        if let Ok(lib) = unsafe { libloading::Library::new(&target) } {
            if let Ok(f) = unsafe { lib.get::<unsafe extern "system" fn() -> i32>(b"DllUnregisterServer") } {
                unsafe { f() };
            }
        }
        // In use by the camera service until it lets go; it's gone after a restart.
        let _ = std::fs::remove_file(&target);
        return Ok(());
    }
    std::fs::create_dir_all(target.parent().unwrap()).context("creating the Program Files folder")?;
    std::fs::copy(source_dll()?, &target).context("copying the camera to Program Files")?;
    // SAFETY: our own DLL's registration entry point.
    unsafe {
        let lib = libloading::Library::new(&target).context("loading the camera")?;
        let f = lib.get::<unsafe extern "system" fn() -> i32>(b"DllRegisterServer").context("the camera has no registration")?;
        let hr = f();
        if hr < 0 {
            bail!("registering the camera failed ({hr:#010x})");
        }
    }
    Ok(())
}

/// The shared memory, once the camera service has made it.
struct View {
    handle: HANDLE,
    base: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl View {
    fn open() -> Option<View> {
        let name = HSTRING::from(shared::MAPPING);
        // SAFETY: plain Win32 calls; the view is unmapped in Drop.
        unsafe {
            let h = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &name)
                .or_else(|_| CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, shared::SIZE as u32, &name))
                .ok()?;
            let base = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, shared::SIZE);
            if base.Value.is_null() {
                let _ = CloseHandle(h);
                return None;
            }
            Some(View { handle: h, base })
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: unmapping and closing what open() made.
        unsafe {
            let _ = UnmapViewOfFile(self.base);
            let _ = CloseHandle(self.handle);
        }
    }
}

struct VirtualCamera {
    stop: Arc<AtomicBool>,
    camera: super::Mta<IMFVirtualCamera>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // SAFETY: plain calls; the camera disappears from apps.
        unsafe {
            let _ = self.camera.0.Stop();
            let _ = self.camera.0.Shutdown();
        }
    }
}

/// Show the program frames from `rx` on the "Cutaway" camera.
pub fn start(rx: Receiver<VideoFrame>) -> Result<Box<dyn Running>> {
    super::mf()?;
    if !installed() {
        bail!("the Cutaway camera isn't installed yet: Record › Screens & webcam › Install…");
    }
    com();
    let name = HSTRING::from("Cutaway");
    let id = HSTRING::from(shared::CLSID_TEXT);
    // SAFETY: plain Media Foundation calls.
    let camera = unsafe {
        let cam = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_Session,
            MFVirtualCameraAccess_CurrentUser,
            &name,
            &id,
            Some(&[KSCATEGORY_VIDEO_CAMERA]),
        )
        .map_err(|e| super::explain(&e, "Creating the Cutaway camera (Windows 11 is needed)"))?;
        cam.Start(None).map_err(|e| super::explain(&e, "Starting the Cutaway camera"))?;
        cam
    };
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("virtual camera".into()).spawn(move || {
        let mut view: Option<View> = None;
        let mut last_try: Option<Instant> = None;
        while !s2.load(Ordering::Relaxed) {
            let frame = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // The camera service makes the shared memory when an app opens
            // the camera: look for it now and then until it's there.
            if view.is_none() && last_try.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
                last_try = Some(Instant::now());
                view = View::open();
            }
            let Some(v) = &view else { continue };
            let f = if (frame.width, frame.height) == (shared::WIDTH, shared::HEIGHT) { frame } else { frame.scaled(shared::WIDTH, shared::HEIGHT) };
            f.pixels.with_planes(|p| {
                let (w, h) = (shared::WIDTH as usize, shared::HEIGHT as usize);
                let tight = p.y_stride == w && p.uv_stride == w;
                let (y, uv) = if tight {
                    (p.y[..w * h].to_vec(), p.uv[..w * h / 2].to_vec())
                } else {
                    let mut y = Vec::with_capacity(w * h);
                    let mut uv = Vec::with_capacity(w * h / 2);
                    for r in 0..h {
                        y.extend_from_slice(&p.y[r * p.y_stride..r * p.y_stride + w]);
                    }
                    for r in 0..h / 2 {
                        uv.extend_from_slice(&p.uv[r * p.uv_stride..r * p.uv_stride + w]);
                    }
                    (y, uv)
                };
                // SAFETY: the view is SIZE bytes, mapped for writing.
                unsafe { shared::write(v.base.Value as *mut u8, &y, &uv, GetTickCount64()) };
            });
        }
    })?;
    Ok(Box::new(VirtualCamera { stop, camera: super::Mta(camera), thread: Some(thread) }))
}
