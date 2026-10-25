//! Stream keys, passwords and tokens kept in the computer's own safe place
//! (the Keychain on Mac, Credential Manager on Windows) instead of in the
//! settings file. The file keeps a placeholder like "@keychain:3f9a01c2b7de".
//!
//! All of them live in ONE entry (a list of placeholder → value), so a Mac
//! asks at most once to let Cutaway read it, even after an update.
//!
//! If the safe place can't be used (no Keychain, or the person said no),
//! values stay in the settings file as before, and nothing is lost.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

pub const PREFIX: &str = "@keychain:";

/// Where the list is kept.
pub trait Store: Send {
    /// The saved text, or None if there isn't any yet.
    fn read(&self) -> Result<Option<String>, String>;
    fn write(&self, text: &str) -> Result<(), String>;
}

pub struct Vault<S: Store> {
    store: S,
    map: Option<BTreeMap<String, String>>,
    /// Reading failed: don't write over what's there this session.
    broken: bool,
}

impl<S: Store> Vault<S> {
    pub fn new(store: S) -> Self {
        Vault { store, map: None, broken: false }
    }

    fn map(&mut self) -> Option<&mut BTreeMap<String, String>> {
        if self.map.is_none() && !self.broken {
            match self.store.read() {
                Ok(text) => {
                    let map = match text {
                        None => BTreeMap::new(),
                        Some(t) => match serde_json::from_str(&t) {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("Saved keys and passwords couldn't be read ({e}); keeping them in the settings file");
                                self.broken = true;
                                return None;
                            }
                        },
                    };
                    self.map = Some(map);
                }
                Err(e) => {
                    eprintln!("Couldn't open the saved keys and passwords ({e}); keeping them in the settings file");
                    self.broken = true;
                }
            }
        }
        self.map.as_mut()
    }

    /// Swap each value for a placeholder before the file is written. An
    /// empty field that couldn't be filled in at load (`held`) gets its
    /// placeholder back, so it isn't lost. If saving to the safe place
    /// fails, values are left as they are (written to the file).
    pub fn hide(&mut self, mut fields: Vec<(String, &mut String)>, held: &[(String, String)]) {
        for (name, v) in fields.iter_mut() {
            if v.is_empty() {
                if let Some((_, p)) = held.iter().find(|(n, _)| n == name) {
                    **v = p.clone();
                }
            }
        }
        let Some(map) = self.map() else { return };
        let mut added = Vec::new();
        let mut ids = Vec::new();
        for (_, v) in fields.iter() {
            if v.trim().is_empty() || v.starts_with(PREFIX) {
                ids.push(None);
                continue;
            }
            let id = match map.iter().find(|(_, x)| x.as_str() == v.as_str()) {
                Some((id, _)) => id.clone(),
                None => {
                    let id = new_id();
                    map.insert(id.clone(), v.to_string());
                    added.push(id.clone());
                    id
                }
            };
            ids.push(Some(id));
        }
        if !added.is_empty() {
            let text = serde_json::to_string(&*map).unwrap_or_default();
            if let Err(e) = self.store.write(&text) {
                eprintln!("Couldn't save keys and passwords safely ({e}); keeping them in the settings file");
                if let Some(map) = self.map.as_mut() {
                    for id in &added {
                        map.remove(id);
                    }
                }
                return;
            }
        }
        for ((_, v), id) in fields.into_iter().zip(ids) {
            if let Some(id) = id {
                *v = format!("{PREFIX}{id}");
            }
        }
    }

    /// Fill in placeholders after the file is read. Returns the ones that
    /// couldn't be filled (left empty), by field name.
    pub fn reveal(&mut self, fields: Vec<(String, &mut String)>) -> Vec<(String, String)> {
        let mut held = Vec::new();
        for (name, v) in fields {
            let Some(id) = v.strip_prefix(PREFIX) else { continue };
            let id = id.trim().to_string();
            match self.map().and_then(|m| m.get(&id)) {
                Some(real) => *v = real.clone(),
                None => {
                    held.push((name, v.clone()));
                    v.clear();
                }
            }
        }
        held
    }

    /// Forget values no settings file uses any more.
    pub fn tidy(&mut self, used: &HashSet<String>) {
        if self.broken {
            return;
        }
        let Some(map) = self.map() else { return };
        let before = map.len();
        map.retain(|id, _| used.contains(id));
        if map.len() != before {
            let text = serde_json::to_string(&*map).unwrap_or_default();
            if let Err(e) = self.store.write(&text) {
                eprintln!("Couldn't tidy saved keys and passwords ({e})");
            }
        }
    }
}

fn new_id() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The placeholder ids in a settings file's text.
pub fn ids_in(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices(PREFIX).map(move |(i, _)| {
        text[i + PREFIX.len()..].chars().take_while(|c| c.is_ascii_hexdigit()).collect()
    })
}

// ---- the computer's own store ----------------------------------------------------------

#[cfg(target_os = "macos")]
mod system {
    use security_framework::passwords;

    const SERVICE: &str = "Cutaway";
    const ACCOUNT: &str = "Settings keys and passwords";
    /// errSecItemNotFound
    const NOT_FOUND: i32 = -25300;

    pub struct System;

    impl super::Store for System {
        fn read(&self) -> Result<Option<String>, String> {
            match passwords::get_generic_password(SERVICE, ACCOUNT) {
                Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) if e.code() == NOT_FOUND => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }
        fn write(&self, text: &str) -> Result<(), String> {
            passwords::set_generic_password(SERVICE, ACCOUNT, text.as_bytes()).map_err(|e| e.to_string())
        }
    }

    pub fn available() -> bool {
        true
    }
}

#[cfg(windows)]
mod system {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    /// A credential holds at most 2560 bytes, so a long list is split.
    const CHUNK: usize = 2000;

    pub struct System;

    fn target(i: usize) -> Vec<u16> {
        format!("Cutaway/settings keys and passwords/{i}").encode_utf16().chain([0]).collect()
    }

    /// One piece, or None if there isn't one.
    fn read_one(i: usize) -> Result<Option<Vec<u8>>, String> {
        let name = target(i);
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe {
            match CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut p) {
                Ok(()) => {
                    let c = &*p;
                    let bytes = if c.CredentialBlob.is_null() {
                        Vec::new()
                    } else {
                        std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
                    };
                    CredFree(p as *const _);
                    Ok(Some(bytes))
                }
                Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
                Err(e) => Err(e.message()),
            }
        }
    }

    impl super::Store for System {
        fn read(&self) -> Result<Option<String>, String> {
            let mut all = Vec::new();
            let mut i = 0;
            while let Some(part) = read_one(i)? {
                all.extend_from_slice(&part);
                i += 1;
            }
            Ok((i > 0).then(|| String::from_utf8_lossy(&all).into_owned()))
        }

        fn write(&self, text: &str) -> Result<(), String> {
            let bytes = text.as_bytes();
            let mut user: Vec<u16> = "Cutaway".encode_utf16().chain([0]).collect();
            let parts: Vec<&[u8]> = if bytes.is_empty() { vec![&[][..]] } else { bytes.chunks(CHUNK).collect() };
            for (i, part) in parts.iter().enumerate() {
                let mut name = target(i);
                let mut blob = part.to_vec();
                let cred = CREDENTIALW {
                    Flags: CRED_FLAGS(0),
                    Type: CRED_TYPE_GENERIC,
                    TargetName: PWSTR(name.as_mut_ptr()),
                    CredentialBlobSize: blob.len() as u32,
                    CredentialBlob: blob.as_mut_ptr(),
                    Persist: CRED_PERSIST_LOCAL_MACHINE,
                    UserName: PWSTR(user.as_mut_ptr()),
                    ..Default::default()
                };
                unsafe { CredWriteW(&cred, 0) }.map_err(|e| e.message())?;
            }
            // Remove pieces left over from a longer list.
            let mut i = parts.len();
            loop {
                let name = target(i);
                if unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) }.is_err() {
                    break;
                }
                i += 1;
            }
            Ok(())
        }
    }

    pub fn available() -> bool {
        true
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod system {
    /// Linux (for development): values stay in the settings file.
    pub struct System;

    impl super::Store for System {
        fn read(&self) -> Result<Option<String>, String> {
            Err("not supported here".into())
        }
        fn write(&self, _: &str) -> Result<(), String> {
            Err("not supported here".into())
        }
    }

    pub fn available() -> bool {
        false
    }
}

static VAULT: Mutex<Option<Vault<system::System>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Vault<system::System>) -> T) -> Option<T> {
    // Tests never touch the real Keychain.
    if cfg!(test) || !system::available() || std::env::var_os("CUTAWAY_KEYS_IN_FILE").is_some() {
        return None;
    }
    let mut v = VAULT.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(v.get_or_insert_with(|| Vault::new(system::System))))
}

/// See `Vault::hide`.
pub fn hide(fields: Vec<(String, &mut String)>, held: &[(String, String)]) {
    // When the store is off, values simply stay as they are.
    let _ = with(|v| v.hide(fields, held));
}

/// See `Vault::reveal`.
pub fn reveal(fields: Vec<(String, &mut String)>) -> Vec<(String, String)> {
    let mut fields = Some(fields);
    if let Some(held) = with(|v| v.reveal(fields.take().unwrap_or_default())) {
        return held;
    }
    // No store here: a placeholder is never used as a key.
    let mut held = Vec::new();
    for (name, v) in fields.unwrap_or_default() {
        if v.starts_with(PREFIX) {
            held.push((name, std::mem::take(v)));
        }
    }
    held
}

/// Forget values that no settings file next to `config_path` (or in its
/// show profiles) uses any more.
pub fn tidy(config_path: &Path) {
    let mut used = HashSet::new();
    let mut files: Vec<_> = vec![config_path.to_path_buf()];
    for dir in [crate::config::profiles_dir(config_path)] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")));
        }
    }
    for f in files {
        match std::fs::read_to_string(&f) {
            Ok(text) => used.extend(ids_in(&text)),
            // Can't tell what it uses: keep everything.
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return,
            Err(_) => {}
        }
    }
    with(|v| v.tidy(&used));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Mem {
        text: Arc<Mutex<Option<String>>>,
        fail_read: bool,
        fail_write: Arc<Mutex<bool>>,
    }

    impl Store for Mem {
        fn read(&self) -> Result<Option<String>, String> {
            if self.fail_read {
                return Err("denied".into());
            }
            Ok(self.text.lock().unwrap().clone())
        }
        fn write(&self, text: &str) -> Result<(), String> {
            if *self.fail_write.lock().unwrap() {
                return Err("full".into());
            }
            *self.text.lock().unwrap() = Some(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn hides_and_reveals() {
        let mem = Mem::default();
        let mut v = Vault::new(mem.clone());
        let (mut a, mut b, mut c) = ("live_abc".to_string(), String::new(), "live_abc".to_string());
        v.hide(vec![("a".into(), &mut a), ("b".into(), &mut b), ("c".into(), &mut c)], &[]);
        assert!(a.starts_with(PREFIX));
        assert_eq!(b, "");
        assert_eq!(a, c, "the same value shares one entry");
        assert!(!mem.text.lock().unwrap().clone().unwrap().is_empty());

        // A fresh run reads it back.
        let mut v = Vault::new(mem.clone());
        let held = v.reveal(vec![("a".into(), &mut a), ("b".into(), &mut b)]);
        assert!(held.is_empty());
        assert_eq!(a, "live_abc");

        // Saving the same value again doesn't add another.
        let mut a2 = "live_abc".to_string();
        v.hide(vec![("a".into(), &mut a2)], &[]);
        assert_eq!(a2, c);
        assert_eq!(v.map.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn missing_values_are_held_not_lost() {
        let mem = Mem { fail_read: true, ..Default::default() };
        let mut v = Vault::new(mem.clone());
        let mut k = format!("{PREFIX}0123456789ab");
        let held = v.reveal(vec![("streaming.destinations.0.key".into(), &mut k)]);
        assert_eq!(k, "", "a placeholder is never used as a key");
        assert_eq!(held.len(), 1);

        // Saving puts the placeholder back; a new value stays in the file
        // (the store couldn't be read, so it isn't written over).
        let mut new = "typed".to_string();
        v.hide(vec![("streaming.destinations.0.key".into(), &mut k), ("x".into(), &mut new)], &held);
        assert_eq!(k, format!("{PREFIX}0123456789ab"));
        assert_eq!(new, "typed");
        assert!(mem.text.lock().unwrap().is_none());
    }

    #[test]
    fn a_failed_write_keeps_values_in_the_file() {
        let mem = Mem::default();
        *mem.fail_write.lock().unwrap() = true;
        let mut v = Vault::new(mem.clone());
        let mut k = "secret".to_string();
        v.hide(vec![("k".into(), &mut k)], &[]);
        assert_eq!(k, "secret");
        assert!(v.map.as_ref().unwrap().is_empty());
    }

    #[test]
    fn tidy_forgets_unused() {
        let mem = Mem::default();
        let mut v = Vault::new(mem.clone());
        let (mut a, mut b) = ("one".to_string(), "two".to_string());
        v.hide(vec![("a".into(), &mut a), ("b".into(), &mut b)], &[]);
        let used: HashSet<String> = ids_in(&format!("key = \"{a}\"\n")).collect();
        v.tidy(&used);
        let mut v = Vault::new(mem);
        let held = v.reveal(vec![("a".into(), &mut a), ("b".into(), &mut b)]);
        assert_eq!(a, "one");
        assert_eq!(held.len(), 1);
    }
}
