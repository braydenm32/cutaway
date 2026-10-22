//! Getting NDI onto a computer that doesn't have it. Cutaway doesn't ship
//! NDI: when someone chooses NDI and it isn't installed, it downloads NDI's
//! own free runtime installer from NDI and opens it, so the person agrees to
//! NDI's licence in NDI's installer. Cutaway picks the runtime up as soon as
//! it's installed (`engine::ndi`).

use crate::util::LockExt;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// NDI's links to its runtime installers.
pub const WINDOWS_URL: &str = "https://ndi.link/NDIRedistV6";
pub const MAC_URL: &str = "https://ndi.link/NDIRedistV6Apple";
/// NDI's own page, for when the download doesn't work.
pub const PAGE: &str = "https://ndi.video/tools/";
/// NDI's licence asks for this near wherever NDI is chosen, with a link to
/// ndi.video.
pub const TRADEMARK: &str = "NDI® is a registered trademark of Vizrt NDI AB.";
pub const SITE: &str = "https://ndi.video";

/// What an installer for each system starts with: a Windows program ("MZ"),
/// or a macOS installer package ("xar!").
const EXE_MAGIC: &[u8] = b"MZ";
const PKG_MAGIC: &[u8] = b"xar!";

/// The installer for this computer: (NDI's link, file name, how it starts),
/// or None where NDI has no runtime installer (Linux: the NDI SDK).
pub fn installer() -> Option<(&'static str, &'static str, &'static [u8])> {
    if cfg!(target_os = "macos") {
        Some((MAC_URL, "NDI Runtime.pkg", PKG_MAGIC))
    } else if cfg!(windows) {
        Some((WINDOWS_URL, "NDI Runtime.exe", EXE_MAGIC))
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Idle,
    Downloading { got: u64, total: Option<u64> },
    /// NDI's installer is open; Cutaway is waiting for NDI to turn up.
    Opened(PathBuf),
    Failed(String),
}

static STAGE: Mutex<Stage> = Mutex::new(Stage::Idle);

pub fn stage() -> Stage {
    STAGE.lock_ok().clone()
}

fn set(s: Stage) {
    *STAGE.lock_ok() = s;
}

/// The installer has been started, so it's worth looking for NDI again.
pub fn waiting() -> bool {
    matches!(stage(), Stage::Downloading { .. } | Stage::Opened(_))
}

/// Download NDI's installer and open it, in the background.
pub fn start() {
    let Some((url, name, magic)) = installer() else {
        set(Stage::Failed("NDI has no installer for this system: install the NDI SDK from ndi.video".into()));
        return;
    };
    {
        let mut s = STAGE.lock_ok();
        if matches!(*s, Stage::Downloading { .. }) {
            return;
        }
        *s = Stage::Downloading { got: 0, total: None };
    }
    let spawned = std::thread::Builder::new().name("ndi download".into()).spawn(move || {
        let to = std::env::temp_dir().join("Cutaway").join(name);
        let r = download(url, &to, magic, &mut |got, total| set(Stage::Downloading { got, total })).and_then(|path| {
            crate::util::open_with_system(&path).map_err(|e| anyhow!("couldn't open NDI's installer ({e})"))?;
            Ok(path)
        });
        match r {
            Ok(path) => {
                crate::logging::note(&format!("NDI: opened its installer ({})", path.display()));
                set(Stage::Opened(path));
            }
            Err(e) => {
                crate::logging::note(&format!("NDI download: {e:#}"));
                set(Stage::Failed(format!("{e:#}")));
            }
        }
    });
    if spawned.is_err() {
        set(Stage::Failed("couldn't start the download".into()));
    }
}

/// NDI is installed: nothing more to do.
pub fn reset() {
    let mut s = STAGE.lock_ok();
    if !matches!(*s, Stage::Downloading { .. }) {
        *s = Stage::Idle;
    }
}

/// Open the downloaded installer again (if it was closed by mistake).
pub fn open_again() {
    if let Stage::Opened(p) = stage() {
        if let Err(e) = crate::util::open_with_system(&p) {
            set(Stage::Failed(format!("couldn't open NDI's installer ({e})")));
        }
    }
}

/// "12 of 48 MB".
pub fn progress_text(got: u64, total: Option<u64>) -> String {
    let mb = |b: u64| b as f64 / 1_000_000.0;
    match total {
        Some(t) if t > 0 => format!("{:.0} of {:.0} MB", mb(got), mb(t)),
        _ => format!("{:.0} MB", mb(got)),
    }
}

/// Fetch `url` (following NDI's redirect) into `to`, by way of a `.part`
/// file, reporting progress. The file must start with `magic`, so an error
/// page is never opened as an installer.
pub fn download(url: &str, to: &Path, magic: &[u8], progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    super::fetch::fetch(url, to, magic, "NDI's website", "its installer", &std::sync::atomic::AtomicBool::new(false), progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// A tiny web server: `/get` redirects to `/file`, which answers with
    /// `body` (claiming `claimed` bytes).
    fn serve(body: Vec<u8>, claimed: usize) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in l.incoming().take(2) {
                let Ok(mut s) = stream else { continue };
                let mut first = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                r.read_line(&mut first).unwrap();
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                if first.starts_with("GET /get ") {
                    let _ = write!(s, "HTTP/1.1 302 Found\r\nLocation: http://{addr}/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                } else {
                    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {claimed}\r\nConnection: close\r\n\r\n");
                    let _ = s.write_all(&body);
                }
            }
        });
        format!("http://{addr}/get")
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cutaway-ndi-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("NDI Runtime.exe")
    }

    #[test]
    fn downloads_the_installer_through_ndis_link() {
        let mut body = b"MZ".to_vec();
        body.extend((0..700_000u32).map(|i| (i % 251) as u8));
        let url = serve(body.clone(), body.len());
        let to = temp("ok");
        let mut seen = Vec::new();
        let got = download(&url, &to, EXE_MAGIC, &mut |g, t| seen.push((g, t))).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), body);
        assert!(!to.with_extension("part").exists());
        assert_eq!(seen.first(), Some(&(0, Some(body.len() as u64))));
        assert_eq!(seen.last(), Some(&(body.len() as u64, Some(body.len() as u64))));
        assert!(seen.len() > 2, "progress as it goes");
    }

    #[test]
    fn never_keeps_a_page_or_a_broken_download() {
        let to = temp("page");
        let url = serve(b"<html>Moved</html>".to_vec(), 18);
        let e = download(&url, &to, EXE_MAGIC, &mut |_, _| {}).unwrap_err();
        assert!(e.to_string().contains("something other"), "{e}");
        assert!(!to.exists() && !to.with_extension("part").exists());

        let to = temp("cut");
        let url = serve(b"MZ only part of it".to_vec(), 5000);
        assert!(download(&url, &to, EXE_MAGIC, &mut |_, _| {}).is_err());
        assert!(!to.exists() && !to.with_extension("part").exists());
    }

    #[test]
    fn words() {
        assert_eq!(progress_text(12_400_000, Some(48_000_000)), "12 of 48 MB");
        assert_eq!(progress_text(3_000_000, None), "3 MB");
        // Each system gets NDI's own installer (none on Linux).
        match installer() {
            Some((url, name, _)) => assert!(url.starts_with("https://ndi.link/") && (name.ends_with(".exe") || name.ends_with(".pkg"))),
            None => assert!(!cfg!(any(target_os = "macos", windows))),
        }
    }
}
