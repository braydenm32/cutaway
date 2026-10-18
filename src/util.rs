//! Small helpers.

pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

pub fn safe_file_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect();
    let t = cleaned.trim().replace(' ', "_");
    if t.is_empty() { "untitled".into() } else { t }
}

/// Run `f`, turning an unexpected internal failure (a panic, or a system
/// framework exception) into an ordinary error, so one misbehaving device
/// can't take the whole app down. The details go to the log file.
pub fn guard<T>(what: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".into());
            // The first line says what happened; the rest (e.g. a system
            // stack trace) is in the log file.
            let first = msg.lines().next().unwrap_or("").trim();
            Err(anyhow::anyhow!("{what} failed unexpectedly: {first}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(safe_file_name("Cam 1/Wide:*"), "Cam_1_Wide__");
        assert_eq!(safe_file_name("  "), "untitled");
    }

    #[test]
    fn guard_turns_panics_into_errors() {
        let r: anyhow::Result<()> = guard("opening Cam 1", || panic!("boom"));
        assert_eq!(r.unwrap_err().to_string(), "opening Cam 1 failed unexpectedly: boom");
        assert_eq!(guard("ok", || Ok(5)).unwrap(), 5);
    }
}

/// A file shipped with the app: inside the macOS bundle's Resources, next
/// to the program, or in the download's folders (e.g. "streamdeck").
pub fn bundled(name: &str, folder: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let mut candidates = vec![dir.join("../Resources").join(name), dir.join(name), dir.join(folder).join(name)];
    // Running from a build folder (target/release) or the download's root.
    for up in dir.ancestors().take(4) {
        candidates.push(up.join(folder).join(name));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(folder).join(name));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Open a file with the app the system uses for it.
pub fn open_with_system(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path).spawn().map(|_| ())
}
