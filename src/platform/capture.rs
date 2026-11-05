//! What a screen-capture source captures: a whole display, one window, or
//! every window of an app; and the list of them (with small pictures) that
//! Settings offers to choose from.
//!
//! In the configuration a screen source's device is:
//! - `screen`: the main display;
//! - `screen:display:<name>`: a display, by the name `displays` gives it;
//! - `screen:window:<app>|<title>`: a window, found again by its app and title
//!   (the title can change a little: the app's window with the closest
//!   title is taken);
//! - `screen:app:<app>`: an app's windows. `<app>` is its bundle id on a Mac
//!   (`com.apple.Keynote`) and its program's file name on Windows
//!   (`POWERPNT.EXE`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenTarget {
    Main,
    Display(String),
    Window { app: String, title: String },
    App(String),
}

impl ScreenTarget {
    /// The target of a source's device, if it's a screen capture.
    pub fn parse(device: &str) -> Option<ScreenTarget> {
        let d = device.trim();
        if d.eq_ignore_ascii_case("screen") {
            return Some(ScreenTarget::Main);
        }
        let rest = d.strip_prefix("screen:")?;
        if let Some(name) = rest.strip_prefix("display:") {
            return Some(if name.trim().is_empty() { ScreenTarget::Main } else { ScreenTarget::Display(name.trim().to_string()) });
        }
        if let Some(w) = rest.strip_prefix("window:") {
            let (app, title) = w.split_once('|').unwrap_or((w, ""));
            return Some(ScreenTarget::Window { app: app.trim().to_string(), title: title.to_string() });
        }
        if let Some(app) = rest.strip_prefix("app:") {
            return Some(ScreenTarget::App(app.trim().to_string()));
        }
        None
    }

    pub fn device(&self) -> String {
        match self {
            ScreenTarget::Main => "screen".into(),
            ScreenTarget::Display(n) => format!("screen:display:{n}"),
            ScreenTarget::Window { app, title } => format!("screen:window:{app}|{title}"),
            ScreenTarget::App(a) => format!("screen:app:{a}"),
        }
    }

    /// In a few words, for lists.
    pub fn describe(&self) -> String {
        match self {
            ScreenTarget::Main => "Main screen".into(),
            ScreenTarget::Display(n) => format!("Screen: {n}"),
            ScreenTarget::Window { app, title } if title.is_empty() => format!("Window of {}", short_app(app)),
            ScreenTarget::Window { title, .. } => format!("Window: {title}"),
            ScreenTarget::App(a) => format!("App: {}", short_app(a)),
        }
    }
}

/// "com.apple.Keynote" → "Keynote", "POWERPNT.EXE" → "POWERPNT".
pub fn short_app(app: &str) -> String {
    let a = app.trim();
    let a = a.strip_suffix(".exe").or_else(|| a.strip_suffix(".EXE")).unwrap_or(a);
    if a.contains('.') && !a.contains(' ') {
        // A bundle id: its last part, unless that's a generic one
        // ("com.spotify.client" → "spotify").
        let parts: Vec<&str> = a.split('.').collect();
        let last = parts[parts.len() - 1];
        if parts.len() > 2 && ["client", "app", "desktop", "mac", "macos", "osx", "application"].contains(&last.to_lowercase().as_str()) {
            parts[parts.len() - 2].to_string()
        } else {
            last.to_string()
        }
    } else {
        a.to_string()
    }
}

/// Whether a device is a screen capture.
/// What a Mac says when Screen Recording hasn't been allowed (the picker
/// spots it to offer System Settings and a restart).
pub const SCREEN_NOT_ALLOWED: &str = "Cutaway isn't allowed to record the screen yet. In System Settings → Privacy & Security → Screen Recording, turn on Cutaway, then restart Cutaway";

/// Open the Screen Recording page of System Settings (Mac).
pub fn open_screen_privacy() {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture").spawn();
}

/// Quit and open Cutaway again (macOS only lets a newly allowed app record
/// the screen after it starts again). Returns false when it can't.
pub fn relaunch_soon() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    // The app bundle when there is one (…/Cutaway.app/Contents/MacOS/cutaway).
    let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(|p| p.to_path_buf());
    #[cfg(target_os = "macos")]
    let r = match &bundle {
        // Once this copy has gone: `open -n` starts a fresh one.
        Some(app) => std::process::Command::new("/bin/sh").arg("-c").arg("sleep 2; open -n \"$0\"").arg(app).spawn(),
        None => std::process::Command::new("/bin/sh").arg("-c").arg("sleep 2; \"$0\" &").arg(&exe).spawn(),
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = {
        let _ = &bundle;
        std::process::Command::new("/bin/sh").arg("-c").arg("sleep 2; \"$0\" &").arg(&exe).spawn()
    };
    // (Windows asks nothing of the kind: no restart needed there.)
    #[cfg(windows)]
    let r: std::io::Result<()> = {
        let _ = (&bundle, &exe);
        Err(std::io::ErrorKind::Unsupported.into())
    };
    r.is_ok()
}

pub fn is_screen(device: &str) -> bool {
    ScreenTarget::parse(device).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Display,
    Window,
    App,
}

/// A small picture of a display or window (RGBA).
#[derive(Clone)]
pub struct Thumb {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// Something that can be captured, as Settings lists it.
#[derive(Clone)]
pub struct CaptureTarget {
    pub kind: TargetKind,
    /// The source device that captures it.
    pub device: String,
    /// The display's name, the window's title or the app's name.
    pub name: String,
    /// The window's app, or the display's size.
    pub detail: String,
    pub thumb: Option<Thumb>,
}

/// Of the windows `(id, title)` an app has open, the one that best fits a
/// remembered title: the same title, else the one sharing the most words
/// with it (a document's name stays while "— Edited" comes and goes), else
/// the first (usually the frontmost).
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub fn best_window<T: Copy>(windows: &[(T, String)], title: &str) -> Option<T> {
    if let Some((id, _)) = windows.iter().find(|(_, t)| t == title) {
        return Some(*id);
    }
    let words = |s: &str| s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(|w| w.to_lowercase()).collect::<Vec<_>>();
    let want = words(title);
    let score = |t: &str| words(t).iter().filter(|w| want.contains(w)).count();
    let best = windows.iter().enumerate().max_by_key(|(i, (_, t))| (score(t), std::cmp::Reverse(*i)));
    best.map(|(_, (id, _))| *id)
}

/// Where a `(w, h)` picture goes, aspect kept, inside a `(fw, fh)` frame:
/// (x, y, w, h), all even (for NV12).
#[cfg_attr(not(windows), allow(dead_code))] // (used by Windows capture)
pub fn fit_inside(w: u32, h: u32, fw: u32, fh: u32) -> (u32, u32, u32, u32) {
    if w == 0 || h == 0 || fw == 0 || fh == 0 {
        return (0, 0, fw & !1, fh & !1);
    }
    let (sw, sh) = if w as u64 * fh as u64 > h as u64 * fw as u64 { (fw, (h as u64 * fw as u64 / w as u64) as u32) } else { ((w as u64 * fh as u64 / h as u64) as u32, fh) };
    let (sw, sh) = (sw.max(2) & !1, sh.max(2) & !1);
    (((fw - sw) / 2) & !1, ((fh - sh) / 2) & !1, sw, sh)
}

/// Where a captured screen picture goes in the output frame: the part of
/// the `tex` texture that holds the picture (`content`, clamped to it, from
/// the top left), the even output size, and the picture's place in it (see
/// [`fit_inside`]). The same for the GPU and CPU conversions, so both look
/// the same.
#[cfg_attr(not(windows), allow(dead_code))] // (used by Windows capture)
pub fn screen_layout(content: (u32, u32), tex: (u32, u32), frame: (u32, u32)) -> ScreenLayout {
    let src = (content.0.max(1).min(tex.0), content.1.max(1).min(tex.1));
    let frame = (frame.0 & !1, frame.1 & !1);
    let (x, y, w, h) = fit_inside(src.0, src.1, frame.0, frame.1);
    ScreenLayout { src, frame, place: (x, y, w, h) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenLayout {
    /// The picture's size in the captured texture (from its top left).
    pub src: (u32, u32),
    /// The output frame's size (even).
    pub frame: (u32, u32),
    /// (x, y, w, h) of the picture in the frame; black around it.
    pub place: (u32, u32, u32, u32),
}

impl ScreenLayout {
    /// The picture fills the whole frame (no black around it).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn fills(&self) -> bool {
        (self.place.2, self.place.3) == self.frame
    }
}

/// Whether CUTAWAY_CPU_SCREEN_CONVERT asks for Windows screen pictures to be
/// converted on the CPU (troubleshooting): set to anything but "" or "0".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn cpu_screen_convert_forced(var: Option<&std::ffi::OsStr>) -> bool {
    var.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Direct3D 11's video processor colour space bits (for drivers without
/// the newer DXGI colour space call): RGB full range in, BT.709 video
/// range out, as [`crate::media::bgra_to_nv12`] does. Bits: Usage 0,
/// RGB_Range 1 (0 = 0-255), YCbCr_Matrix 2 (1 = BT.709), xvYCC 3,
/// Nominal_Range 4-5 (1 = 16-235).
#[cfg_attr(not(windows), allow(dead_code))]
pub const VP_COLOR_SPACE_709_VIDEO: u32 = (1 << 2) | (1 << 4);

/// Put an NV12 picture (`y`, `uv`, `w`×`h`) on black in a `fw`×`fh` frame
/// at `(x, y)` (all even).
#[cfg_attr(not(windows), allow(dead_code))] // (used by Windows capture)
pub fn place_nv12(y_src: &[u8], uv_src: &[u8], w: usize, h: usize, fw: usize, fh: usize, x: usize, y: usize) -> (Vec<u8>, Vec<u8>) {
    let mut yp = vec![16u8; fw * fh];
    let mut uvp = vec![128u8; fw * fh / 2];
    for row in 0..h.min(fh.saturating_sub(y)) {
        let n = w.min(fw.saturating_sub(x));
        yp[(y + row) * fw + x..][..n].copy_from_slice(&y_src[row * w..][..n]);
    }
    for row in 0..(h / 2).min(fh.saturating_sub(y) / 2) {
        let n = w.min(fw.saturating_sub(x));
        uvp[(y / 2 + row) * fw + x..][..n].copy_from_slice(&uv_src[row * w..][..n]);
    }
    (yp, uvp)
}

/// Shrink an RGBA (or BGRA) picture to at most `max` wide or high, by
/// averaging (for thumbnails).
#[cfg_attr(not(windows), allow(dead_code))] // (used by Windows capture)
pub fn shrink_rgba(src: &[u8], w: usize, h: usize, stride: usize, max: usize) -> Thumb {
    let k = (w.max(h) as f32 / max as f32).max(1.0);
    let (tw, th) = (((w as f32 / k) as usize).max(1), ((h as f32 / k) as usize).max(1));
    let mut out = vec![0u8; tw * th * 4];
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            // A few samples per cell are plenty for a thumbnail.
            let (sx, sy) = (((x1 - x0) / 3).max(1), ((y1 - y0) / 3).max(1));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            let mut yy = y0;
            while yy < y1 {
                let mut xx = x0;
                while xx < x1 {
                    let p = &src[yy * stride + xx * 4..][..4];
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                    n += 1;
                    xx += sx;
                }
                yy += sy;
            }
            let o = &mut out[(ty * tw + tx) * 4..][..4];
            for c in 0..4 {
                o[c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    Thumb { size: [tw, th], rgba: out }
}

/// Swap B and R (BGRA → RGBA) and make it opaque.
#[cfg_attr(not(windows), allow(dead_code))] // (used by Windows capture)
pub fn bgra_to_rgba(t: &mut Thumb) {
    for p in t.rgba.chunks_exact_mut(4) {
        p.swap(0, 2);
        p[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_layout_matches_the_cpu_placement() {
        // 4K screen into a 1080p show: fills it.
        let l = screen_layout((3840, 2160), (3840, 2160), (1920, 1080));
        assert_eq!(l, ScreenLayout { src: (3840, 2160), frame: (1920, 1080), place: (0, 0, 1920, 1080) });
        assert!(l.fills());
        // 16:10 screen into 16:9: black at the sides, all even.
        let l = screen_layout((2560, 1600), (2560, 1600), (1920, 1080));
        assert_eq!(l.place, (96, 0, 1728, 1080));
        assert!(!l.fills());
        // A window smaller than its texture (the pool not resized yet): only
        // the picture's part, clamped to the texture.
        let l = screen_layout((801, 3000), (1000, 600), (1281, 721));
        assert_eq!(l.src, (801, 600));
        assert_eq!(l.frame, (1280, 720));
        assert_eq!(l.place, fit_inside(801, 600, 1280, 720));
        let (x, y, w, h) = l.place;
        assert!(x % 2 == 0 && y % 2 == 0 && w % 2 == 0 && h % 2 == 0);
        assert!(x + w <= 1280 && y + h <= 720);
        // No size reported yet: never a zero-sized source.
        assert_eq!(screen_layout((0, 0), (640, 480), (640, 480)).src, (1, 1));
    }

    #[test]
    fn cpu_screen_convert_env() {
        use std::ffi::OsStr;
        assert!(!cpu_screen_convert_forced(None));
        assert!(!cpu_screen_convert_forced(Some(OsStr::new(""))));
        assert!(!cpu_screen_convert_forced(Some(OsStr::new("0"))));
        assert!(cpu_screen_convert_forced(Some(OsStr::new("1"))));
        assert!(cpu_screen_convert_forced(Some(OsStr::new("yes"))));
    }

    #[test]
    fn video_processor_color_space_bits() {
        // RGB full range, BT.709 matrix, 16-235: D3D11's bit layout.
        assert_eq!(VP_COLOR_SPACE_709_VIDEO & 0b10, 0);
        assert_eq!((VP_COLOR_SPACE_709_VIDEO >> 2) & 1, 1);
        assert_eq!((VP_COLOR_SPACE_709_VIDEO >> 4) & 0b11, 1);
        assert_eq!(VP_COLOR_SPACE_709_VIDEO, 0x14);
    }

    #[test]
    fn devices_round_trip() {
        for t in [
            ScreenTarget::Main,
            ScreenTarget::Display("DELL U2720Q (2)".into()),
            ScreenTarget::Window { app: "com.apple.Keynote".into(), title: "Show | final.key".into() },
            ScreenTarget::App("POWERPNT.EXE".into()),
        ] {
            assert_eq!(ScreenTarget::parse(&t.device()), Some(t.clone()));
        }
        assert_eq!(ScreenTarget::parse("Screen"), Some(ScreenTarget::Main));
        assert_eq!(ScreenTarget::parse("screen:display:"), Some(ScreenTarget::Main));
        assert_eq!(ScreenTarget::parse("FaceTime HD Camera"), None);
        assert!(!is_screen("screens of doom"));
    }

    #[test]
    fn app_names_read_well() {
        assert_eq!(short_app("com.apple.Keynote"), "Keynote");
        assert_eq!(short_app("POWERPNT.EXE"), "POWERPNT");
        assert_eq!(short_app("chrome.exe"), "chrome");
        assert_eq!(short_app("Google Chrome"), "Google Chrome");
        assert_eq!(short_app("com.spotify.client"), "spotify");
    }

    #[test]
    fn finds_the_window_again() {
        let w = vec![(1, "Untitled".to_string()), (2, "Budget 2026.xlsx — Edited".to_string()), (3, "Notes".to_string())];
        assert_eq!(best_window(&w, "Notes"), Some(3));
        assert_eq!(best_window(&w, "Budget 2026.xlsx"), Some(2));
        assert_eq!(best_window(&w, "Something else"), Some(1));
        assert_eq!(best_window::<u32>(&[], "x"), None);
    }

    #[test]
    fn fits_with_bars() {
        // A tall window in a 16:9 frame: bars left and right.
        assert_eq!(fit_inside(1000, 1000, 1920, 1080), (420, 0, 1080, 1080));
        // A wide one: bars top and bottom.
        assert_eq!(fit_inside(2000, 500, 1920, 1080), (0, 300, 1920, 480));
        assert_eq!(fit_inside(1920, 1080, 1920, 1080), (0, 0, 1920, 1080));
    }

    #[test]
    fn places_a_picture_on_black() {
        let (y, uv) = place_nv12(&[200; 4], &[50; 2], 2, 2, 6, 4, 2, 2);
        assert_eq!(y[2 * 6 + 2], 200);
        assert_eq!(y[0], 16);
        assert_eq!(uv[6 + 2], 50);
        assert_eq!(uv[0], 128);
    }

    #[test]
    fn thumbnails_shrink() {
        let src = vec![255u8; 400 * 300 * 4];
        let t = shrink_rgba(&src, 400, 300, 400 * 4, 100);
        assert_eq!(t.size, [100, 75]);
        assert!(t.rgba.iter().all(|&v| v == 255));
    }
}
