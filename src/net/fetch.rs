//! Downloading a file: into a `.part` file first, with progress, checked to
//! be what was asked for (not an error page), and stoppable.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Fetch `url` (following redirects) into `to`, reporting progress. The file
/// must start with `magic`. `site` and `what` name things in errors ("NDI's
/// website", "its installer"). Setting `cancel` stops it.
pub fn fetch(url: &str, to: &Path, magic: &[u8], site: &str, what: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let agent = crate::util::https_agent(Duration::from_secs(30 * 60));
    let mut resp = agent.get(url).header("User-Agent", concat!("Cutaway/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| anyhow!("couldn't reach {site} ({e})"))?;
    let status = resp.status().as_u16();
    if status != 200 {
        bail!("{site} answered with error {status}");
    }
    let total = resp.body().content_length();
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    }
    let part = to.with_extension("part");
    let r = (|| -> Result<u64> {
        let mut file = std::fs::File::create(&part).with_context(|| format!("couldn't write {}", part.display()))?;
        let mut reader = resp.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        progress(0, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("stopped");
            }
            let n = reader.read(&mut buf).map_err(|e| anyhow!("the download was cut off ({e})"))?;
            if n == 0 {
                break;
            }
            // The start is checked once there's enough of it (the first
            // read can be short).
            if (got as usize) < magic.len() {
                let had = got as usize;
                let upto = (magic.len() - had).min(n);
                if buf[..upto] != magic[had..had + upto] {
                    bail!("{site} sent something other than {what}");
                }
            }
            file.write_all(&buf[..n]).context("couldn't save the download")?;
            got += n as u64;
            progress(got, total);
        }
        file.flush()?;
        Ok(got)
    })();
    let got = match r {
        Ok(got) => got,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if got == 0 || total.is_some_and(|t| t != got) {
        let _ = std::fs::remove_file(&part);
        bail!("the download was cut off");
    }
    std::fs::rename(&part, to).with_context(|| format!("couldn't save {}", to.display()))?;
    Ok(to.to_path_buf())
}
