//! Sound from one app (Syrinscape, Spotify, a browser, a game…), captured
//! with Windows' process loopback (Windows 10 2004 and Windows 11): no
//! virtual audio cable needed. The app's child processes are included, so
//! browsers that play sound from a helper process work too.

use super::com;
use crate::engine::sources::AudioStarted;
use crate::media::{clock, AudioBlock, NS_PER_SEC};
use crate::platform::AudioSink;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{implement, Interface, Ref, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{IAgileObject, IAgileObject_Impl, BLOB};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{CreateEventW, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::Variant::VT_BLOB;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible};
use windows::core::BOOL;

/// Running processes: pid → executable name without ".exe".
fn processes() -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    // SAFETY: a snapshot handle we close; entries are plain structs.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(&name).to_string();
            out.insert(e.th32ProcessID, stem);
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

unsafe extern "system" fn add_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the set passed to EnumWindows below.
    let pids = unsafe { &mut *(lparam.0 as *mut HashSet<u32>) };
    unsafe {
        if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            pids.insert(pid);
        }
    }
    BOOL(1)
}

/// Apps with a window, by name.
pub fn list_apps() -> Vec<String> {
    let mut pids: HashSet<u32> = HashSet::new();
    // SAFETY: the callback only touches the set it's given.
    let _ = unsafe { EnumWindows(Some(add_window), LPARAM(&mut pids as *mut _ as isize)) };
    let me = std::process::id();
    let procs = processes();
    let mut names: Vec<String> = pids.iter().filter(|p| **p != me).filter_map(|p| procs.get(p).cloned()).filter(|n| !n.eq_ignore_ascii_case("explorer")).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// The main process of an app: the one whose parent isn't the same app
/// (browsers and Electron apps run many).
fn find_process(name: &str) -> Option<u32> {
    let want = name.trim().trim_end_matches(".exe").to_lowercase();
    let procs = processes();
    let matching: Vec<u32> = procs.iter().filter(|(_, n)| n.to_lowercase() == want).map(|(p, _)| *p).collect();
    if matching.is_empty() {
        return None;
    }
    // Prefer one with a window.
    let mut windows: HashSet<u32> = HashSet::new();
    // SAFETY: as in list_apps.
    let _ = unsafe { EnumWindows(Some(add_window), LPARAM(&mut windows as *mut _ as isize)) };
    matching.iter().copied().find(|p| windows.contains(p)).or(matching.first().copied())
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Completion {
    done: Mutex<Option<mpsc::Sender<()>>>,
}

impl IAgileObject_Impl for Completion_Impl {}

impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Some(tx) = self.done.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

/// An audio client capturing `pid` and everything it starts.
fn loopback_client(pid: u32) -> Result<IAudioClient> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS { TargetProcessId: pid, ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE },
        },
    };
    let mut pv = PROPVARIANT::default();
    // SAFETY: a VT_BLOB pointing at `params`, which outlives the call.
    unsafe {
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob = BLOB { cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, pBlobData: &params as *const _ as *mut u8 };
    }
    let (tx, rx) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Completion { done: Mutex::new(Some(tx)) }.into();
    // SAFETY: plain call; the handler is agile and the params live until it completes.
    let op = unsafe { ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&pv), &handler) }
        .map_err(|e| anyhow!("Windows didn't allow listening to one app ({e}); it needs Windows 11 or Windows 10 version 2004"))?;
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("Windows didn't answer"))?;
    let mut hr = HRESULT(0);
    let mut unknown = None;
    // SAFETY: valid out pointers.
    unsafe { op.GetActivateResult(&mut hr, &mut unknown) }?;
    hr.ok()?;
    Ok(unknown.ok_or_else(|| anyhow!("no audio client"))?.cast()?)
}

struct AppRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for AppRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Still running?
fn alive(process: HANDLE) -> bool {
    // SAFETY: a process handle we own.
    unsafe { WaitForSingleObject(process, 0) == WAIT_TIMEOUT }
}

pub fn open_app_audio(name: &str, sink: AudioSink) -> Result<AudioStarted> {
    let pid = find_process(name).ok_or_else(|| anyhow!("{name} isn't running"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("app sound".into()).spawn(move || {
        com();
        let setup = || -> Result<(IAudioClient, IAudioCaptureClient, HANDLE, HANDLE)> {
            let client = loopback_client(pid)?;
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
                nChannels: 2,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 48_000 * 8,
                nBlockAlign: 8,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            // SAFETY: plain WASAPI calls on the client just made.
            unsafe {
                client
                    .Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, 2_000_000, 0, &format, None)
                    .context("setting up the capture")?;
                let event = CreateEventW(None, false, false, None)?;
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
                client.Start()?;
                Ok((client, capture, event, process))
            }
        };
        let (client, capture, event, process) = match setup() {
            Ok(x) => {
                let _ = ready_tx.send(Ok(()));
                x
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let block = 10_000_000i64;
        let mut last_real = clock::now();
        let mut silence_at: Option<i64> = None;
        let mut checked = std::time::Instant::now();
        let mut running = true;
        while !s2.load(Ordering::Relaxed) {
            // SAFETY: our own event handle.
            let woke = unsafe { WaitForSingleObject(event, 20) } == WAIT_OBJECT_0;
            if woke {
                loop {
                    // SAFETY: plain calls; the buffer is released after copying.
                    let packet = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
                    if packet == 0 {
                        break;
                    }
                    let (mut data, mut frames, mut flags, mut qpc) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                    if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc)) }.is_err() {
                        break;
                    }
                    let n = frames as usize * 2;
                    let samples = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        vec![0.0f32; n]
                    } else {
                        unsafe { std::slice::from_raw_parts(data as *const f32, n) }.to_vec()
                    };
                    let _ = unsafe { capture.ReleaseBuffer(frames) };
                    let now = clock::now();
                    let mut pts = clock::from_100ns(qpc as i64);
                    if (pts - now).abs() > 5 * NS_PER_SEC {
                        pts = now - frames as i64 * NS_PER_SEC / 48_000;
                    }
                    last_real = now;
                    silence_at = None;
                    sink(AudioBlock { pts, rate: 48_000, channels: 2, samples });
                }
            }
            if checked.elapsed() > Duration::from_secs(2) {
                checked = std::time::Instant::now();
                running = alive(process);
                if !running {
                    break; // the app quit: the input reconnects when it's back
                }
            }
            // A quiet app sends nothing: fill with silence so the input
            // doesn't look disconnected.
            let now = clock::now();
            if running && now - last_real > NS_PER_SEC / 5 {
                let mut t = silence_at.unwrap_or(now - block);
                while t + block <= now {
                    sink(AudioBlock { pts: t, rate: 48_000, channels: 2, samples: vec![0.0; 960] });
                    t += block;
                }
                silence_at = Some(t);
            }
        }
        // SAFETY: stopping and closing what setup made.
        unsafe {
            let _ = client.Stop();
            let _ = CloseHandle(event);
            let _ = CloseHandle(process);
        }
    })?;
    match ready_rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!("listening to {name}: {e:#}"),
        Err(_) => bail!("listening to {name} didn't start"),
    }
    Ok(AudioStarted { running: Box::new(AppRunning { stop, thread: Some(thread) }), description: format!("sound from {name}"), rate: 48_000, channels: 2 })
}
