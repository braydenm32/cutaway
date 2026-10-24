//! Small helpers.

/// Standard base64 (as in JSON and data: addresses) back to bytes; None if
/// it isn't base64.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8]),
            2 => out.push((n >> 16) as u8),
            _ => return None,
        }
    }
    Some(out)
}

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

/// CRC-32 (as zip files use it).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A zip file holding `files` uncompressed (small project files).
pub fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date (1980-01-01)
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
                h.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// An HTTPS client that trusts the system's certificates (Syrinscape,
/// model and tool downloads).
pub fn https_agent(timeout: std::time::Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder().tls_config(tls).timeout_connect(Some(std::time::Duration::from_secs(20))).timeout_global(Some(timeout)).http_status_as_error(false).build().into()
}

/// Locking that survives a crash elsewhere. When a background part panics
/// while holding a lock, Rust marks the lock "poisoned" and every later
/// `lock().unwrap()` panics too, so one failure spreads through the app.
/// `lock_ok` takes the lock anyway: the data inside is still usable (at worst
/// it's missing the crashed part's last change), and the crash itself is
/// already in the log.
pub trait LockExt<T: ?Sized> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for std::sync::Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
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
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b+c%41", true), "a b cA");
        assert_eq!(percent_decode("a+b", false), "a+b");
        assert_eq!(percent_decode("100%", true), "100%");
        assert_eq!(percent_decode("%é%4", true), "%é%4");
        assert_eq!(percent_decode("%C3%A9", true), "é");
    }

    #[test]
    fn truncating() {
        assert_eq!(truncate_str("héllo", 2), "h");
        assert_eq!(truncate_str("héllo", 3), "hé");
        assert_eq!(truncate_str("hi", 10), "hi");
    }

    #[test]
    fn lock_survives_a_crash_while_held() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let mut g = m2.lock_ok();
            *g = 2;
            panic!("a background part crashes mid-change");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*m.lock_ok(), 2);
    }

    #[test]
    fn crc_and_zip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let z = zip_stored(&[("a.txt", b"hello"), ("b/c.xml", b"<x/>")]);
        assert_eq!(&z[..4], b"PK\x03\x04");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
    }

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

/// `s` without `prefix`, ignoring ASCII case ("NDI:Cam" and "ndi:Cam" alike).
pub fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// The first `max` bytes of `s` at most, cut on a character boundary.
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

/// Undo URL %-escapes (and `+` for a space in form data). Bad escapes are
/// kept as they are.
pub fn percent_decode(s: &str, plus_is_space: bool) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' if plus_is_space => out.push(b' '),
            b'%' => match (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
