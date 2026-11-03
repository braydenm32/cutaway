//! Signed updates. The updater already checks that a download matches the
//! release's fingerprints (SHA256SUMS.txt). That proves the file isn't
//! damaged, not that it came from Cutaway's maker: anyone who got into the
//! GitHub release could swap both. So the fingerprints are signed too, with
//! a key that is kept off line (never on GitHub), and the app only installs
//! an update whose fingerprints carry that signature.
//!
//! - `cutaway --make-update-key <file>` makes the key once and prints its
//!   public half, which goes in [`PUBLIC_KEY`].
//! - `cutaway --sign-update <SHA256SUMS.txt> --key <file>` writes
//!   `SHA256SUMS.txt.sig` beside it, to add to the release before it is
//!   published.

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::path::{Path, PathBuf};

/// The signature's file name in a release.
pub const SIG: &str = "SHA256SUMS.txt.sig";

/// The public half of the update key, in hex. While it's empty, updates
/// don't need a signature (as before); once it's set, every update must be
/// signed with the matching private key.
pub const PUBLIC_KEY: &str = "a5866fcaf643c85a2753012dac6a4af53bcb86bd0e51ab153ede7bc046eef425";

/// The key updates are checked against: [`PUBLIC_KEY`] (a test can stand
/// in a key of its own, so it can sign its own pretend releases).
fn built_in() -> String {
    #[cfg(test)]
    if let Some(k) = TEST_KEY.with(|k| k.borrow().clone()) {
        return k;
    }
    PUBLIC_KEY.to_string()
}

#[cfg(test)]
thread_local! {
    /// A test's own public key, in place of [`PUBLIC_KEY`] on this thread.
    pub static TEST_KEY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Whether updates must be signed.
pub fn required() -> bool {
    !built_in().trim().is_empty()
}

const KEY_NOTE: &str = "Cutaway update key. Keep this file private and off line (for example on a USB stick and in a password manager): anyone who has it can sign updates that every copy of Cutaway will install. If it's lost, updates can't be signed until a new key is built into the app.";

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn public_key(hex_key: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = unhex(hex_key).and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("the update key built into Cutaway isn't valid"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow!("the update key built into Cutaway isn't valid"))
}

/// Check `sig` (the text of SHA256SUMS.txt.sig) against `sums` with a
/// public key.
pub fn verify_with(public_hex: &str, sums: &[u8], sig: &str) -> Result<()> {
    let key = public_key(public_hex)?;
    let bytes: [u8; 64] = sig.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).and_then(unhex).and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("the update's signature is unreadable, so it wasn't installed"))?;
    key.verify(sums, &Signature::from_bytes(&bytes)).map_err(|_| anyhow!("the update isn't signed by Cutaway's maker, so it wasn't installed"))
}

/// Check a release's fingerprints, when updates must be signed.
pub fn verify(sums: &[u8], sig: Option<&str>) -> Result<()> {
    if !required() {
        return Ok(());
    }
    let Some(sig) = sig else {
        bail!("the update isn't signed, so it wasn't installed");
    };
    verify_with(&built_in(), sums, sig)
}

/// Make a new key at `path` (never over an existing file). Returns the
/// public half, in hex.
pub fn make_key(path: &Path) -> Result<String> {
    if path.exists() {
        bail!("{} already exists; pick another name (a key is only made once)", path.display());
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow!("couldn't get random numbers: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    let public = hex(key.verifying_key().as_bytes());
    let text = format!("# {KEY_NOTE}\n# Public key: {public}\n{}\n", hex(&seed));
    write_private(path, &text).with_context(|| format!("couldn't write {}", path.display()))?;
    Ok(public)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

fn read_key(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("couldn't read the key {}", path.display()))?;
    let seed: [u8; 32] = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(unhex)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!("{} isn't a Cutaway update key", path.display()))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The signature text for `sums`.
pub fn signature(key: &SigningKey, sums: &[u8]) -> String {
    format!("{}\n", hex(&key.sign(sums).to_bytes()))
}

/// Sign a SHA256SUMS.txt: writes SHA256SUMS.txt.sig beside it. Says if the
/// key doesn't match the one built into this copy.
pub fn sign_file(sums: &Path, key: &Path) -> Result<(PathBuf, Option<String>)> {
    let key = read_key(key)?;
    let data = std::fs::read(sums).with_context(|| format!("couldn't read {}", sums.display()))?;
    if data.is_empty() {
        bail!("{} is empty", sums.display());
    }
    let out = sums.with_file_name(SIG);
    std::fs::write(&out, signature(&key, &data)).with_context(|| format!("couldn't write {}", out.display()))?;
    let public = hex(key.verifying_key().as_bytes());
    let note = (required() && public != PUBLIC_KEY.trim()).then(|| format!("this key ({public}) isn't the one built into this copy of Cutaway, so this copy wouldn't accept it"));
    Ok((out, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key built into this version is a real Ed25519 public key.
    #[test]
    fn built_in_key_is_valid() {
        assert!(required(), "this version checks update signatures");
        public_key(PUBLIC_KEY).unwrap();
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(unhex("0f0"), None);
        assert_eq!(unhex("zz"), None);
    }

    /// A signed list passes; a changed list, another key's signature and
    /// a missing signature don't (review G15).
    #[test]
    fn only_the_makers_signature_passes() {
        let dir = std::env::temp_dir().join(format!("cutaway-sign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key_file = dir.join("update.key");
        let public = make_key(&key_file).unwrap();
        assert!(make_key(&key_file).is_err(), "never overwrites a key");
        let sums = dir.join("SHA256SUMS.txt");
        std::fs::write(&sums, "abc  Cutaway-1.0.0-macos-arm64.zip\n").unwrap();
        let (sig_file, _) = sign_file(&sums, &key_file).unwrap();
        assert_eq!(sig_file.file_name().unwrap(), SIG);
        let sig = std::fs::read_to_string(&sig_file).unwrap();
        let data = std::fs::read(&sums).unwrap();
        verify_with(&public, &data, &sig).unwrap();
        assert!(verify_with(&public, b"abd  Cutaway-1.0.0-macos-arm64.zip\n", &sig).is_err());
        let other = SigningKey::from_bytes(&[7; 32]);
        assert!(verify_with(&public, &data, &signature(&other, &data)).is_err());
        assert!(verify_with(&public, &data, "not a signature").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
