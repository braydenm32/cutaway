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
