//! A log file, so problems can be diagnosed when Cutaway runs as an app
//! (with no terminal to print to), and a full record of any crash.
//!
//! macOS: ~/Library/Logs/Cutaway/Cutaway.log (also visible in Console.app).
//! Windows: %LOCALAPPDATA%\Cutaway\Logs\Cutaway.log.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static PATH: OnceLock<PathBuf> = OnceLock::new();
/// Standard error is the log file (launched as an app, not from a terminal).
static STDERR_IS_LOG: AtomicBool = AtomicBool::new(false);

fn default_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        crate::config::home_dir().join("Library/Logs/Cutaway/Cutaway.log")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::config::home_dir().join("AppData").join("Local"))
            .join("Cutaway")
            .join("Logs")
            .join("Cutaway.log")
    } else {
        crate::config::default_config_path().with_file_name("cutaway.log")
    }
}

pub fn path() -> PathBuf {
    PATH.get().cloned().unwrap_or_else(default_path)
}

fn append(text: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path()) {
        let _ = writeln!(f, "{text}");
    }
}

/// Set up the log file and the crash handler. Call first thing.
pub fn init() {
    let path = default_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Keep one previous log; don't grow forever.
    if fs::metadata(&path).map(|m| m.len() > 5_000_000).unwrap_or(false) {
        let _ = fs::rename(&path, path.with_extension("previous.log"));
    }
    let _ = PATH.set(path.clone());

    // Started from Finder/Dock or the Start menu: everything normally
    // printed goes to the file.
    #[cfg(windows)]
    {
        use std::io::IsTerminal;
        if !std::io::stderr().is_terminal() {
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                use std::os::windows::io::IntoRawHandle;
                let h = windows::Win32::Foundation::HANDLE(f.into_raw_handle());
                // SAFETY: the handle stays open for the life of the process.
                if unsafe { windows::Win32::System::Console::SetStdHandle(windows::Win32::System::Console::STD_ERROR_HANDLE, h) }.is_ok() {
                    STDERR_IS_LOG.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    #[cfg(unix)]
    unsafe {
        if libc::isatty(2) == 0 {
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                use std::os::fd::AsRawFd;
                if libc::dup2(f.as_raw_fd(), 2) >= 0 {
                    STDERR_IS_LOG.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    let banner = format!(
        "\n=== Cutaway {} started {} ({} {}) ===",
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if STDERR_IS_LOG.load(Ordering::Relaxed) {
        eprintln!("{banner}");
    } else {
        append(&banner);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = format!("INTERNAL ERROR in thread '{thread}': {info}\n{backtrace}");
        if STDERR_IS_LOG.load(Ordering::Relaxed) {
            eprintln!("{text}");
        } else {
            append(&text);
            default_hook(info);
        }
    }));
}

/// Write a line to the log (for problems outside the engine's own log,
/// e.g. a controller sending a command Cutaway doesn't know).
pub fn note(text: &str) {
    let line = format!("{} {text}", chrono::Local::now().format("%H:%M:%S"));
    if STDERR_IS_LOG.load(Ordering::Relaxed) {
        eprintln!("{line}");
    } else {
        append(&line);
    }
}

/// Show the log file in Finder (or the folder in the file manager).
pub fn reveal() {
    let p = path();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(&p).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    if let Some(dir) = p.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
