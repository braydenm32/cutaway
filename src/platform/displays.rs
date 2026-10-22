//! The computer's displays: their names, where they sit and how big they
//! are, in the order the windowing system counts them (so an index here is
//! the index a window can be sent full screen to).

/// One display.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Its name ("DELL U2720Q", "Built-in Retina Display"); displays with
    /// the same name get " (2)", " (3)"… so each is told apart.
    pub name: String,
    /// Where it is in the desktop's arrangement, in window units (points
    /// on a Mac, pixels on Windows; see [`window_units`]).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Its picture size in pixels.
    pub pixels: (u32, u32),
    /// The main display (the one with the menu bar or the taskbar).
    pub main: bool,
    /// The system's id for it (CGDirectDisplayID on a Mac, the HMONITOR on
    /// Windows): good until it's unplugged.
    pub id: u64,
}

impl Display {
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] < self.x + self.w && p[1] >= self.y && p[1] < self.y + self.h
    }

    pub fn center(&self) -> [f32; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }
}

/// A window's position (in points, unzoomed, as egui reports it) in the
/// units [`Display`] uses.
pub fn window_units(p: [f32; 2], native_pixels_per_point: f32) -> [f32; 2] {
    if cfg!(windows) {
        [p[0] * native_pixels_per_point, p[1] * native_pixels_per_point]
    } else {
        p
    }
}

/// Every display, in the windowing system's order. Empty where it can't
/// be asked (Linux), which means "wherever the window opens".
pub fn list() -> Vec<Display> {
    let mut v = imp::list();
    number_duplicates(&mut v);
    v
}

/// [`list`], asked again at most every two seconds (for drawing it in
/// settings every frame).
pub fn list_recent() -> Vec<Display> {
    use crate::util::LockExt;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<Display>)>> = Mutex::new(None);
    let mut c = CACHE.lock_ok();
    match &*c {
        Some((at, v)) if at.elapsed() < Duration::from_secs(2) => v.clone(),
        _ => {
            let v = list();
            *c = Some((Instant::now(), v.clone()));
            v
        }
    }
}

/// The display a point (in window units) is on.
pub fn containing(displays: &[Display], p: [f32; 2]) -> Option<usize> {
    displays.iter().position(|d| d.contains(p))
}

/// The display for the second-screen output: the one named `wanted` if
/// it's there; otherwise another display than the one Cutaway's window is
/// on (`ours`; when that's unknown, another than the main display). None:
/// there's no other display.
pub fn pick_output(displays: &[Display], wanted: &str, ours: Option<usize>) -> Option<usize> {
    if !wanted.is_empty() {
        if let Some(i) = displays.iter().position(|d| d.name == wanted) {
            return Some(i);
        }
    }
    displays.iter().enumerate().position(|(i, d)| match ours {
        Some(o) => i != o,
        None => !d.main,
    })
}

/// Whether the output can open full screen there without covering
/// Cutaway's own window (which would leave nothing to press).
pub fn output_can_fill(displays: &[Display], target: Option<usize>, ours: Option<usize>) -> bool {
    match (target, ours) {
        (Some(t), Some(o)) => t != o,
        (Some(t), None) => !displays[t].main,
        _ => false,
    }
}

fn number_duplicates(v: &mut [Display]) {
    let names: Vec<String> = v.iter().map(|d| d.name.clone()).collect();
    for i in 0..v.len() {
        let same = names.iter().filter(|n| **n == names[i]).count();
        if same > 1 {
            let k = names[..i].iter().filter(|n| **n == names[i]).count() + 1;
            v[i].name = format!("{} ({k})", names[i]);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Display;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    use objc2_core_graphics::{CGDisplayBounds, CGDisplayIsBuiltin, CGDisplayPixelsHigh, CGDisplayPixelsWide, CGGetActiveDisplayList, CGMainDisplayID};
    use objc2_foundation::{NSNumber, NSString};

    pub fn list() -> Vec<Display> {
        let mut ids = [0u32; 32];
        let mut n = 0u32;
        // SAFETY: the buffer holds 32 ids and the count says how many came.
        if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) }.0 != 0 {
            return Vec::new();
        }
        let names = screen_names();
        let main = CGMainDisplayID();
        ids[..n as usize]
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                let b = CGDisplayBounds(id);
                let name = names.iter().find(|(sid, _)| *sid == id).map(|(_, n)| n.clone()).unwrap_or_else(|| {
                    if CGDisplayIsBuiltin(id) {
                        "Built-in display".to_string()
                    } else {
                        format!("Display {}", i + 1)
                    }
                });
                Display {
                    name,
                    x: b.origin.x as f32,
                    y: b.origin.y as f32,
                    w: b.size.width as f32,
                    h: b.size.height as f32,
                    pixels: (CGDisplayPixelsWide(id) as u32, CGDisplayPixelsHigh(id) as u32),
                    main: id == main,
                    id: id as u64,
                }
            })
            .collect()
    }

    /// The names macOS shows in Displays settings, by display id (AppKit
    /// only answers on the main thread; elsewhere the names are generic).
    fn screen_names() -> Vec<(u32, String)> {
        let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
        let key = NSString::from_str("NSScreenNumber");
        NSScreen::screens(mtm)
            .iter()
            .filter_map(|s| {
                let desc = s.deviceDescription();
                let n = desc.objectForKey(&key)?;
                let n = n.downcast::<NSNumber>().ok()?;
                Some((n.unsignedIntValue(), s.localizedName().to_string()))
            })
            .collect()
    }
}

#[cfg(windows)]
mod imp {
    use super::Display;
    use windows::core::BOOL;
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};

    const MONITORINFOF_PRIMARY: u32 = 1;

    pub fn list() -> Vec<Display> {
        unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
            // SAFETY: `out` is the Vec passed below, alive for the call.
            unsafe { (*(out.0 as *mut Vec<HMONITOR>)).push(m) };
            true.into()
        }
        let mut monitors: Vec<HMONITOR> = Vec::new();
        // SAFETY: the callback only pushes onto the Vec, during this call.
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut monitors as *mut _ as isize)) };
        let names = friendly_names();
        monitors
            .iter()
            .enumerate()
            .filter_map(|(i, &m)| {
                let mut info = MONITORINFOEXW::default();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                // SAFETY: the struct is sized for the extended info.
                if !unsafe { GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
                    return None;
                }
                let r = info.monitorInfo.rcMonitor;
                let device = wide(&info.szDevice);
                let name = names.iter().find(|(d, _)| *d == device).map(|(_, n)| n.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Display {}", i + 1));
                let (w, h) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
                Some(Display {
                    name,
                    x: r.left as f32,
                    y: r.top as f32,
                    w: w as f32,
                    h: h as f32,
                    pixels: (w as u32, h as u32),
                    main: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                    id: m.0 as u64,
                })
            })
            .collect()
    }

    fn wide(s: &[u16]) -> String {
        String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
    }

    /// Each display's own name (as in Settings → Display), by its GDI
    /// device name (\\.\DISPLAY1).
    fn friendly_names() -> Vec<(String, String)> {
        // SAFETY: buffers sized from GetDisplayConfigBufferSizes; each
        // request packet is the struct its header says it is.
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
                return Vec::new();
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
                return Vec::new();
            }
            paths.truncate(np as usize);
            paths
                .iter()
                .filter_map(|p| {
                    let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                    src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: p.sourceInfo.adapterId,
                        id: p.sourceInfo.id,
                    };
                    if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                        return None;
                    }
                    let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
                    tgt.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: p.targetInfo.adapterId,
                        id: p.targetInfo.id,
                    };
                    let name = if DisplayConfigGetDeviceInfo(&mut tgt.header) == 0 { wide(&tgt.monitorFriendlyDeviceName) } else { String::new() };
                    let name = if name.is_empty() && tgt.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL { "Built-in display".to_string() } else { name };
                    Some((wide(&src.viewGdiDeviceName), name))
                })
                .collect()
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn list() -> Vec<super::Display> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, x: f32) -> Display {
        Display { name: name.into(), x, y: 0.0, w: 100.0, h: 50.0, pixels: (100, 50), main: false, id: 0 }
    }

    #[test]
    fn same_names_are_numbered() {
        let mut v = vec![d("DELL", 0.0), d("Built-in", 100.0), d("DELL", 200.0)];
        number_duplicates(&mut v);
        let names: Vec<_> = v.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["DELL (1)", "Built-in", "DELL (2)"]);
    }

    #[test]
    fn the_output_goes_on_another_display() {
        let mut v = vec![d("A", 0.0), d("B", 100.0), d("C", 200.0)];
        v[0].main = true;
        // Cutaway on B: the first other one.
        assert_eq!(pick_output(&v, "", Some(1)), Some(0));
        assert!(output_can_fill(&v, Some(0), Some(1)));
        // Chosen by name, even Cutaway's own: then it opens in a window.
        assert_eq!(pick_output(&v, "B", Some(1)), Some(1));
        assert!(!output_can_fill(&v, Some(1), Some(1)));
        // A name that's gone: automatic.
        assert_eq!(pick_output(&v, "Z", Some(0)), Some(1));
        // Not knowing where Cutaway is: not the main display.
        assert_eq!(pick_output(&v, "", None), Some(1));
        // One display: nowhere else to go, and never full screen over Cutaway.
        let one = vec![d("A", 0.0)];
        assert_eq!(pick_output(&one, "", Some(0)), None);
        assert!(!output_can_fill(&one, None, Some(0)));
    }

    #[test]
    fn finds_the_display_a_point_is_on() {
        let v = vec![d("A", 0.0), d("B", 100.0)];
        assert_eq!(containing(&v, [150.0, 10.0]), Some(1));
        assert_eq!(containing(&v, [50.0, 10.0]), Some(0));
        assert_eq!(containing(&v, [250.0, 10.0]), None);
    }
}
