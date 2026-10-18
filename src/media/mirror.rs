//! A file written to two places at once: the main recording and an optional
//! backup copy on another drive. If the backup fails (drive full or
//! unplugged), it's dropped and the main file carries on; the reason is
//! kept for the log.

use anyhow::{Context, Result};
use std::fs::{File, Metadata};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct MirrorFile {
    main: File,
    backup: Option<(File, PathBuf)>,
    backup_error: Option<String>,
}

impl MirrorFile {
    /// Create `path`, plus `backup` if given. A backup that can't be created
    /// is noted (see [`MirrorFile::take_backup_error`]) but isn't an error.
    pub fn create(path: &Path, backup: Option<&Path>) -> Result<MirrorFile> {
        let main = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut m = MirrorFile { main, backup: None, backup_error: None };
        if let Some(b) = backup {
            match File::create(b) {
                Ok(f) => m.backup = Some((f, b.to_path_buf())),
                Err(e) => m.backup_error = Some(format!("couldn't create the backup {}: {e}", b.display())),
            }
        }
        Ok(m)
    }

    fn on_backup(&mut self, op: impl FnOnce(&mut File) -> io::Result<()>) {
        if let Some((f, p)) = &mut self.backup {
            if let Err(e) = op(f) {
                self.backup_error = Some(format!("backup {} stopped: {e}", p.display()));
                self.backup = None;
            }
        }
    }

    /// A problem with the backup copy since last asked.
    pub fn take_backup_error(&mut self) -> Option<String> {
        self.backup_error.take()
    }

    #[cfg(test)]
    pub fn has_backup(&self) -> bool {
        self.backup.is_some()
    }

    pub fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.main.set_len(len)?;
        self.on_backup(|f| f.set_len(len));
        Ok(())
    }

    pub fn sync_data(&mut self) -> io::Result<()> {
        self.main.sync_data()?;
        self.on_backup(|f| f.sync_data());
        Ok(())
    }

    pub fn sync_all(&mut self) -> io::Result<()> {
        self.main.sync_all()?;
        self.on_backup(|f| f.sync_all());
        Ok(())
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        self.main.metadata()
    }
}

impl Write for MirrorFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.main.write_all(buf)?;
        self.on_backup(|f| f.write_all(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.main.flush()?;
        self.on_backup(|f| f.flush());
        Ok(())
    }
}

impl Seek for MirrorFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let at = self.main.seek(pos)?;
        self.on_backup(|f| f.seek(SeekFrom::Start(at)).map(|_| ()));
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_both_and_survives_a_failed_backup() {
        let dir = std::env::temp_dir().join(format!("cutaway-mirror-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.bin"), dir.join("b.bin"));
        let mut m = MirrorFile::create(&a, Some(&b)).unwrap();
        m.write_all(b"hello world").unwrap();
        m.seek(SeekFrom::Start(0)).unwrap();
        m.write_all(b"H").unwrap();
        m.sync_all().unwrap();
        drop(m);
        assert_eq!(std::fs::read(&a).unwrap(), b"Hello world");
        assert_eq!(std::fs::read(&b).unwrap(), b"Hello world");

        // A backup folder that doesn't exist: the main file still works.
        let mut m = MirrorFile::create(&dir.join("c.bin"), Some(&dir.join("missing/c.bin"))).unwrap();
        assert!(m.take_backup_error().is_some());
        m.write_all(b"x").unwrap();
        assert!(!m.has_backup());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
