//! The COM objects Windows' camera service talks to.

use crate::shared;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::{implement, w, ComObject, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    LocalFree, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, ERROR_SET_NOT_FOUND, E_POINTER, HANDLE, HLOCAL, HMODULE, INVALID_HANDLE_VALUE, S_FALSE, S_OK, TRUE,
};
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, OpenFileMappingW, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};
use windows::Win32::System::SystemInformation::GetTickCount64;

const CLSID: GUID = GUID::from_u128(shared::CLSID_U128);

/// This DLL, for registering it.
static MODULE: AtomicIsize = AtomicIsize::new(0);

// ---- DLL entry points ------------------------------------------------------------------

#[no_mangle]
extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == 1 {
        MODULE.store(module.0 as isize, Ordering::Relaxed);
    }
    TRUE
}

/// # Safety
/// Called by COM with valid pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    unsafe {
        *ppv = std::ptr::null_mut();
        if rclsid.is_null() || *rclsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    }
}

/// Stays loaded once loaded (the camera service keeps it anyway).
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

/// `regsvr32 cutaway_vcam.dll` (as administrator): register the camera's
/// COM class for this DLL's location. Cutaway does the same itself.
#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register(true) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    match register(false) {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

fn register(add: bool) -> Result<()> {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows::Win32::System::Registry::*;
    let key = format!("SOFTWARE\\Classes\\CLSID\\{}", shared::CLSID_TEXT);
    unsafe {
        if !add {
            return RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(key.as_str())).ok();
        }
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::Relaxed) as *mut c_void)), &mut buf) as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        let set = |sub: &str, name: Option<&str>, value: &str| -> Result<()> {
            let mut h = HKEY::default();
            RegCreateKeyExW(HKEY_LOCAL_MACHINE, &HSTRING::from(sub), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut h, None).ok()?;
            let data: Vec<u8> = value.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect();
            let name = name.map(HSTRING::from);
            let r = RegSetValueExW(h, name.as_ref().map(|n| PCWSTR(n.as_ptr())).unwrap_or(PCWSTR::null()), None, REG_SZ, Some(&data)).ok();
            let _ = RegCloseKey(h);
            r
        };
        set(&key, None, "Cutaway Camera Source")?;
        set(&format!("{key}\\InprocServer32"), None, &path)?;
        set(&format!("{key}\\InprocServer32"), Some("ThreadingModel"), "Both")?;
    }
    Ok(())
}

// ---- class factory -------------------------------------------------------------------------

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activate: IMFActivate = Activate { attrs: attributes()?, source: Mutex::new(None) }.into();
        unsafe { activate.query(riid, ppv).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

fn attributes() -> Result<IMFAttributes> {
    let mut a = None;
    unsafe { MFCreateAttributes(&mut a, 4)? };
    a.ok_or_else(|| E_POINTER.into())
}

// ---- activation object ---------------------------------------------------------------------

/// What the camera service creates first; it makes the media source.
#[implement(IMFActivate)]
struct Activate {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSourceEx>>,
}

impl IMFActivate_Impl for Activate_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        let mut s = self.source.lock().map_err(|_| windows::core::Error::from(E_POINTER))?;
        if s.is_none() {
            *s = Some(new_source()?);
        }
        unsafe { s.as_ref().unwrap().query(riid, ppv).ok() }
    }

    fn ShutdownObject(&self) -> Result<()> {
        if let Ok(mut s) = self.source.lock() {
            if let Some(src) = s.take() {
                let _ = unsafe { src.Shutdown() };
            }
        }
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        if let Ok(mut s) = self.source.lock() {
            *s = None;
        }
        Ok(())
    }
}

/// Everything about attributes goes to the inner store.
impl IMFAttributes_Impl for Activate_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.GetItem(k, (!v.is_null()).then_some(v)) }
    }
    fn GetItemType(&self, k: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attrs.GetItemType(k) }
    }
    fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<BOOL> {
        unsafe { self.attrs.CompareItem(k, v) }
    }
    fn Compare(&self, theirs: Ref<IMFAttributes>, t: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
        unsafe { self.attrs.Compare(theirs.as_ref(), t) }
    }
    fn GetUINT32(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetUINT32(k) }
    }
    fn GetUINT64(&self, k: *const GUID) -> Result<u64> {
        unsafe { self.attrs.GetUINT64(k) }
    }
    fn GetDouble(&self, k: *const GUID) -> Result<f64> {
        unsafe { self.attrs.GetDouble(k) }
    }
    fn GetGUID(&self, k: *const GUID) -> Result<GUID> {
        unsafe { self.attrs.GetGUID(k) }
    }
    fn GetStringLength(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetStringLength(k) }
    }
    fn GetString(&self, k: *const GUID, value: PWSTR, size: u32, len: *mut u32) -> Result<()> {
        if value.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { self.attrs.GetString(k, std::slice::from_raw_parts_mut(value.0, size as usize), (!len.is_null()).then_some(len)) }
    }
    fn GetAllocatedString(&self, k: *const GUID, value: *mut PWSTR, len: *mut u32) -> Result<()> {
        unsafe { self.attrs.GetAllocatedString(k, value, len) }
    }
    fn GetBlobSize(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetBlobSize(k) }
    }
    fn GetBlob(&self, k: *const GUID, buf: *mut u8, size: u32, got: *mut u32) -> Result<()> {
        if buf.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { self.attrs.GetBlob(k, std::slice::from_raw_parts_mut(buf, size as usize), (!got.is_null()).then_some(got)) }
    }
    fn GetAllocatedBlob(&self, k: *const GUID, buf: *mut *mut u8, size: *mut u32) -> Result<()> {
        unsafe { self.attrs.GetAllocatedBlob(k, buf, size) }
    }
    fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        unsafe { (Interface::vtable(&self.attrs).GetUnknown)(Interface::as_raw(&self.attrs), k, riid, ppv).ok() }
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.SetItem(k, v) }
    }
    fn DeleteItem(&self, k: *const GUID) -> Result<()> {
        unsafe { self.attrs.DeleteItem(k) }
    }
    fn DeleteAllItems(&self) -> Result<()> {
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> Result<()> {
        unsafe { self.attrs.SetUINT32(k, v) }
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> Result<()> {
        unsafe { self.attrs.SetUINT64(k, v) }
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> Result<()> {
        unsafe { self.attrs.SetDouble(k, v) }
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> Result<()> {
        unsafe { self.attrs.SetGUID(k, v) }
    }
    fn SetString(&self, k: *const GUID, v: &PCWSTR) -> Result<()> {
        unsafe { self.attrs.SetString(k, *v) }
    }
    fn SetBlob(&self, k: *const GUID, buf: *const u8, size: u32) -> Result<()> {
        if buf.is_null() && size > 0 {
            return Err(E_POINTER.into());
        }
        let data = if size == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(buf, size as usize) } };
        unsafe { self.attrs.SetBlob(k, data) }
    }
    fn SetUnknown(&self, k: *const GUID, v: Ref<IUnknown>) -> Result<()> {
        unsafe { self.attrs.SetUnknown(k, v.as_ref()) }
    }
    fn LockStore(&self) -> Result<()> {
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> Result<()> {
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> Result<u32> {
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.GetItemByIndex(i, k, (!v.is_null()).then_some(v)) }
    }
    fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> Result<()> {
        unsafe { self.attrs.CopyAllItems(dest.as_ref()) }
    }
}

// ---- media source --------------------------------------------------------------------------

#[derive(Default)]
struct SourceState {
    announced: bool,
    shutdown: bool,
}

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
struct Source {
    queue: IMFMediaEventQueue,
    attrs: IMFAttributes,
    pd: IMFPresentationDescriptor,
    sd: IMFStreamDescriptor,
    stream: Mutex<Option<ComObject<Stream>>>,
    state: Mutex<SourceState>,
}

/// The one picture format offered: 1080p30 NV12.
fn media_type() -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((shared::WIDTH as u64) << 32) | shared::HEIGHT as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((shared::FPS as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, shared::WIDTH)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, shared::FRAME_BYTES as u32)?;
        Ok(t)
    }
}

fn new_source() -> Result<IMFMediaSourceEx> {
    unsafe {
        let mt = media_type()?;
        let sd = MFCreateStreamDescriptor(0, &[Some(mt.clone())])?;
        sd.GetMediaTypeHandler()?.SetCurrentMediaType(&mt)?;
        sd.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        sd.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
        sd.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        sd.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;
        let pd = MFCreatePresentationDescriptor(Some(&[Some(sd.clone())]))?;
        pd.SelectStream(0)?;
        let source = ComObject::new(Source {
            queue: MFCreateEventQueue()?,
            attrs: attributes()?,
            pd,
            sd: sd.clone(),
            stream: Mutex::new(None),
            state: Mutex::new(SourceState::default()),
        });
        let stream = ComObject::new(Stream {
            source: Mutex::new(Some(source.to_interface::<IMFMediaSourceEx>().cast::<IMFMediaSource>()?)),
            sd,
            queue: MFCreateEventQueue()?,
            state: Mutex::new(MF_STREAM_STATE_STOPPED),
            frames: Mutex::new(Frames::default()),
        });
        if let Ok(mut s) = source.stream.lock() {
            *s = Some(stream);
        }
        Ok(source.to_interface())
    }
}

fn zero() -> GUID {
    GUID::zeroed()
}

impl Source_Impl {
    fn live(&self) -> Result<()> {
        match self.state.lock() {
            Ok(s) if !s.shutdown => Ok(()),
            _ => Err(MF_E_SHUTDOWN.into()),
        }
    }

    fn stream(&self) -> Result<ComObject<Stream>> {
        self.stream.lock().ok().and_then(|s| s.clone()).ok_or_else(|| MF_E_SHUTDOWN.into())
    }
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        unsafe { self.queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, value: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, value) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        self.live()?;
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.live()?;
        unsafe { self.pd.Clone() }
    }

    fn Start(&self, _pd: Ref<IMFPresentationDescriptor>, format: *const GUID, start: *const PROPVARIANT) -> Result<()> {
        self.live()?;
        if !format.is_null() && unsafe { *format } != zero() {
            return Err(MF_E_UNSUPPORTED_TIME_FORMAT.into());
        }
        let stream = self.stream()?;
        let announced = {
            let mut st = self.state.lock().map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))?;
            std::mem::replace(&mut st.announced, true)
        };
        let s: IMFMediaStream = stream.to_interface::<IMFMediaStream2>().cast()?;
        let met = if announced { MEUpdatedStream } else { MENewStream };
        unsafe {
            self.queue.QueueEventParamUnk(met.0 as u32, &zero(), S_OK, &s)?;
        }
        stream.set_running(true)?;
        unsafe { self.queue.QueueEventParamVar(MESourceStarted.0 as u32, &zero(), S_OK, start) }
    }

    fn Stop(&self) -> Result<()> {
        self.live()?;
        self.stream()?.set_running(false)?;
        let empty = PROPVARIANT::default();
        unsafe { self.queue.QueueEventParamVar(MESourceStopped.0 as u32, &zero(), S_OK, &empty) }
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        if let Ok(mut st) = self.state.lock() {
            st.shutdown = true;
        }
        if let Some(stream) = self.stream.lock().ok().and_then(|mut s| s.take()) {
            stream.shut();
        }
        unsafe {
            let _ = self.queue.Shutdown();
        }
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        self.live()?;
        Ok(self.attrs.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        self.live()?;
        if id != 0 {
            return Err(MF_E_INVALIDSTREAMNUMBER.into());
        }
        self.sd.cast()
    }

    fn SetD3DManager(&self, _manager: Ref<IUnknown>) -> Result<()> {
        // Pictures are in ordinary memory.
        Ok(())
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _service: *const GUID, _riid: *const GUID, _ppv: *mut *mut c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

/// No camera controls (focus, exposure…) on a virtual camera.
impl IKsControl_Impl for Source_Impl {
    fn KsProperty(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }
    fn KsMethod(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }
    fn KsEvent(&self, _p: *const KSIDENTIFIER, _pl: u32, _d: *mut c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        Err(ERROR_SET_NOT_FOUND.to_hresult().into())
    }
}

// ---- the video stream ---------------------------------------------------------------------

/// The shared memory Cutaway writes pictures into.
struct Frames {
    view: Option<(HANDLE, usize)>,
    last_try: u64,
    buf: Vec<u8>,
}

// The view is only touched under the stream's lock.
unsafe impl Send for Frames {}

impl Default for Frames {
    fn default() -> Self {
        Frames { view: None, last_try: 0, buf: vec![0; shared::FRAME_BYTES] }
    }
}

impl Frames {
    /// Make (or open) the shared memory, at most once a second.
    fn open(&mut self, now: u64) {
        if self.view.is_some() || now.saturating_sub(self.last_try) < 1000 {
            return;
        }
        self.last_try = now;
        unsafe {
            // Readable and writable by everyone: Cutaway runs as the user,
            // this runs in the camera service.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let sddl = w!("D:(A;;GA;;;WD)(A;;GA;;;SY)(A;;GA;;;LS)S:(ML;;NW;;;LW)");
            let attrs = ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None)
                .ok()
                .map(|_| SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() });
            let name = HSTRING::from(shared::MAPPING);
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, attrs.as_ref().map(|a| a as *const _), PAGE_READWRITE, 0, shared::SIZE as u32, &name)
                .or_else(|_| OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &name));
            if !sd.0.is_null() {
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
            if let Ok(h) = handle {
                let v = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, shared::SIZE);
                if !v.Value.is_null() {
                    self.view = Some((h, v.Value as usize));
                }
            }
        }
    }

    /// The newest picture from Cutaway, or a dark one.
    fn picture(&mut self) -> &[u8] {
        let now = unsafe { GetTickCount64() };
        self.open(now);
        let fresh = match self.view {
            Some((_, base)) => unsafe { shared::read(base as *const u8, &mut self.buf, now, 1500) },
            None => false,
        };
        if !fresh {
            let y = (shared::WIDTH * shared::HEIGHT) as usize;
            self.buf[..y].fill(24);
            self.buf[y..].fill(128);
        }
        &self.buf
    }
}

#[implement(IMFMediaStream2)]
struct Stream {
    source: Mutex<Option<IMFMediaSource>>,
    sd: IMFStreamDescriptor,
    queue: IMFMediaEventQueue,
    state: Mutex<MF_STREAM_STATE>,
    frames: Mutex<Frames>,
}

impl Stream {
    fn set_running(&self, on: bool) -> Result<()> {
        if let Ok(mut s) = self.state.lock() {
            *s = if on { MF_STREAM_STATE_RUNNING } else { MF_STREAM_STATE_STOPPED };
        }
        let empty = PROPVARIANT::default();
        let met = if on { MEStreamStarted } else { MEStreamStopped };
        unsafe { self.queue.QueueEventParamVar(met.0 as u32, &zero(), S_OK, &empty) }
    }

    fn shut(&self) {
        if let Ok(mut s) = self.source.lock() {
            *s = None; // breaks the source ↔ stream cycle
        }
        unsafe {
            let _ = self.queue.Shutdown();
        }
    }

    fn sample(&self) -> Result<IMFSample> {
        let mut f = self.frames.lock().map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))?;
        let data = f.picture();
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(10_000_000 / shared::FPS as i64)?;
            Ok(sample)
        }
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        unsafe { self.queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, value: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, value) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.source.lock().ok().and_then(|s| s.clone()).ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        Ok(self.sd.clone())
    }

    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        if self.source.lock().ok().is_none_or(|s| s.is_none()) {
            return Err(MF_E_SHUTDOWN.into());
        }
        let sample = self.sample()?;
        unsafe {
            if let Some(t) = token.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, t)?;
            }
            self.queue.QueueEventParamUnk(MEMediaSample.0 as u32, &zero(), S_OK, &sample)
        }
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        if let Ok(mut s) = self.state.lock() {
            *s = value;
        }
        Ok(())
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        self.state.lock().map(|s| *s).map_err(|_| MF_E_SHUTDOWN.into())
    }
}
