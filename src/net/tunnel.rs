//! Gives the guest page a public https:// address with no router changes,
//! one of two ways:
//! - a Cloudflare quick tunnel (…trycloudflare.com): nothing to set up, no
//!   account, but a new address every time. Cutaway runs Cloudflare's
//!   `cloudflared` program, downloading it the first time if it isn't
//!   installed.
//! - Tailscale Funnel (…ts.net): an address that stays the same, so links
//!   can be sent days ahead. Needs Tailscale installed and signed in; Cutaway
//!   runs `tailscale funnel` for as long as the guest page is on.

use crate::util::LockExt;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the tunnel is doing, for the settings and the guests panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TunnelState {
    #[default]
    Off,
    /// Downloading cloudflared / waiting for an address.
    Starting(String),
    Up(String),
    Failed(String),
    /// Waiting on something only the user can do: what, and the page to do
    /// it on (Tailscale asking for Funnel to be allowed, say).
    NeedsYou(String, String),
}

/// Which way the guest page is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Cloudflare,
    Tailscale,
}

pub struct Tunnel {
    child: Arc<Mutex<Option<Child>>>,
    pub state: Arc<Mutex<TunnelState>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    }
}

/// Where Cutaway keeps its own copy.
pub fn bundled_path() -> PathBuf {
    let config = crate::config::default_config_path();
    config.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("bin").join(exe_name())
}

/// cloudflared on this computer: Cutaway's copy, or one installed already.
pub fn find() -> Option<PathBuf> {
    let own = bundled_path();
    if own.exists() {
        return Some(own);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    // Homebrew isn't on a Mac app's PATH.
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(pf) = std::env::var_os("ProgramFiles(x86)").or_else(|| std::env::var_os("ProgramFiles")) {
        dirs.push(PathBuf::from(pf).join("cloudflared"));
    }
    dirs.into_iter().map(|d| d.join(exe_name())).find(|p| p.is_file())
}

/// The release to download for this computer.
fn download_url() -> Result<String> {
    let base = "https://github.com/cloudflare/cloudflared/releases/latest/download/";
    let file = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "cloudflared-darwin-arm64.tgz",
        ("macos", _) => "cloudflared-darwin-amd64.tgz",
        ("windows", "aarch64") => "cloudflared-windows-arm64.exe",
        ("windows", _) => "cloudflared-windows-amd64.exe",
        ("linux", "aarch64") => "cloudflared-linux-arm64",
        ("linux", _) => "cloudflared-linux-amd64",
        (os, arch) => bail!("there's no cloudflared for {os} on {arch}"),
    };
    Ok(format!("{base}{file}"))
}

/// Download cloudflared into Cutaway's folder.
pub fn download(progress: &dyn Fn(String)) -> Result<PathBuf> {
    let url = download_url()?;
    progress("Downloading Cloudflare's tunnel program (about 20 MB, once)…".into());
    let agent = crate::util::https_agent(Duration::from_secs(120));
    let mut resp = agent.get(&url).call().map_err(|e| anyhow!("couldn't download cloudflared: {e}"))?;
    let bytes = resp.body_mut().with_config().limit(200 << 20).read_to_vec().map_err(|e| anyhow!("couldn't download cloudflared: {e}"))?;
    let dest = bundled_path();
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let tmp = dest.with_extension("download");
    if url.ends_with(".tgz") {
        // A gzipped tar with the one program in it.
        std::fs::write(&tmp, &bytes)?;
        let dir = dest.parent().unwrap();
        let status = Command::new("tar").arg("-xzf").arg(&tmp).arg("-C").arg(dir).status().context("unpacking cloudflared")?;
        let _ = std::fs::remove_file(&tmp);
        if !status.success() || !dest.exists() {
            bail!("couldn't unpack cloudflared");
        }
    } else {
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &dest)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dest)
}

/// The tunnel's address in a line cloudflared printed, if there is one.
pub fn address_in(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let rest = &line[i..];
    let end = rest.find(|c: char| c.is_whitespace() || c == '|' || c == '"').unwrap_or(rest.len());
    let url = &rest[..end];
    (url.ends_with(".trycloudflare.com") && !url.contains("api.trycloudflare.com")).then(|| url.to_string())
}

impl Tunnel {
    /// Start a tunnel to `http://127.0.0.1:port` in the background.
    pub fn start(port: u16, provider: Provider, waker: Arc<dyn Fn() + Send + Sync>) -> Tunnel {
        let state = Arc::new(Mutex::new(TunnelState::Starting("Starting the tunnel…".into())));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s2, c2, stop2) = (state.clone(), child.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("tunnel".into()).spawn(move || {
            let set = |st: TunnelState| {
                *s2.lock_ok() = st;
                waker();
            };
            let mut tries = 0;
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                tries += 1;
                let run = match provider {
                    Provider::Cloudflare => run_once(port, &c2, &stop2, &|st| set(st)),
                    Provider::Tailscale => run_funnel(port, &c2, &stop2, &|st| set(st)),
                };
                match run {
                    Ok(()) => {}
                    Err(e) => set(TunnelState::Failed(format!("{e:#}"))),
                }
                if stop2.load(std::sync::atomic::Ordering::Relaxed) || tries > 20 {
                    break;
                }
                // cloudflared stopped (network change, sleep): try again.
                std::thread::sleep(Duration::from_secs(if tries < 3 { 2 } else { 10 }));
            }
        });
        Tunnel { child, state, stop }
    }

    pub fn address(&self) -> Option<String> {
        match &*self.state.lock_ok() {
            TunnelState::Up(a) => Some(a.clone()),
            _ => None,
        }
    }
}

fn run_once(port: u16, child_slot: &Arc<Mutex<Option<Child>>>, stop: &std::sync::atomic::AtomicBool, set: &dyn Fn(TunnelState)) -> Result<()> {
    let exe = match find() {
        Some(p) => p,
        None => download(&|m| set(TunnelState::Starting(m)))?,
    };
    set(TunnelState::Starting("Opening the tunnel…".into()));
    let mut cmd = Command::new(&exe);
    cmd.args(["tunnel", "--no-autoupdate", "--url", &format!("http://127.0.0.1:{port}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().with_context(|| format!("couldn't start {}", exe.display()))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no output from cloudflared"))?;
    *child_slot.lock_ok() = Some(child);
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        kill(child_slot);
        return Ok(());
    }
    let mut last = String::new();
    let mut up = false;
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if !up {
            if let Some(a) = address_in(&line) {
                up = true;
                set(TunnelState::Up(a));
                continue;
            }
            if line.contains(" ERR ") {
                last = line.split(" ERR ").nth(1).unwrap_or("").trim().to_string();
            } else if line.starts_with("failed") {
                last = line.trim().to_string();
            }
        }
    }
    // cloudflared exited.
    kill(child_slot);
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    if last.contains("no such host") || last.contains("lookup") {
        bail!("couldn't reach Cloudflare (is this computer online?): {last}");
    }
    bail!("the tunnel closed{}", if last.is_empty() { String::new() } else { format!(": {last}") })
}

/// The `tailscale` command on this computer: the Mac app (whose program
/// doubles as the command), Homebrew's, Windows' or one on the PATH.
pub fn tailscale_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "tailscale.exe" } else { "tailscale" };
    let mut found: Vec<PathBuf> = vec![PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale")];
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(pf).join("Tailscale"));
    }
    found.extend(dirs.into_iter().map(|d| d.join(name)));
    found.into_iter().find(|p| p.is_file())
}

/// What a line `tailscale funnel` printed means.
#[derive(Debug, PartialEq)]
enum FunnelLine {
    /// The public address (…ts.net), once it's serving.
    Address(String),
    /// A page on Tailscale's site where something has to be allowed first.
    Allow(String),
    Problem(String),
    Other,
}

fn funnel_line(line: &str) -> FunnelLine {
    let line = line.trim().trim_start_matches("|--").trim();
    if let Some(i) = line.find("https://") {
        let rest = &line[i..];
        let url = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
        if url.starts_with("https://login.tailscale.com/") {
            return FunnelLine::Allow(url.to_string());
        }
        let host = url.trim_start_matches("https://").split(['/', ':']).next().unwrap_or("");
        if host.ends_with(".ts.net") {
            return FunnelLine::Address(format!("https://{host}"));
        }
    }
    let lower = line.to_lowercase();
    if lower.contains("logged out") || lower.contains("needslogin") || lower.contains("not logged in") {
        return FunnelLine::Problem("Tailscale isn't signed in: open Tailscale and sign in".into());
    }
    if lower.contains("failed to connect to local tailscale") || lower.contains("is tailscale running") || lower.contains("tailscaled") && lower.contains("not running") {
        return FunnelLine::Problem("Tailscale isn't running: open the Tailscale app".into());
    }
    if lower.starts_with("error") || lower.contains("error:") {
        return FunnelLine::Problem(line.to_string());
    }
    FunnelLine::Other
}

fn run_funnel(port: u16, child_slot: &Arc<Mutex<Option<Child>>>, stop: &std::sync::atomic::AtomicBool, set: &dyn Fn(TunnelState)) -> Result<()> {
    let Some(exe) = tailscale_exe() else {
        bail!("Tailscale isn't installed: get it from tailscale.com/download, sign in, then Cutaway connects by itself");
    };
    set(TunnelState::Starting("Opening Tailscale Funnel…".into()));
    let mut cmd = Command::new(&exe);
    // In the foreground: Funnel stops when Cutaway stops it (or quits).
    cmd.args(["funnel", "--yes", "--https=443", &format!("http://127.0.0.1:{port}")]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().with_context(|| format!("couldn't start {}", exe.display()))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    for out in [child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    *child_slot.lock_ok() = Some(child);
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        kill(child_slot);
        return Ok(());
    }
    let mut last = String::new();
    let mut up = false;
    // Both outputs closed: tailscale exited.
    for line in rx {
        match funnel_line(&line) {
            FunnelLine::Address(a) if !up => {
                up = true;
                set(TunnelState::Up(a));
            }
            FunnelLine::Allow(url) if !up => {
                set(TunnelState::NeedsYou("Tailscale asks you to allow Funnel for your network (once). Cutaway carries on by itself after that.".into(), url));
            }
            FunnelLine::Problem(p) => last = p,
            _ => {
                let l = line.to_lowercase();
                if !up && (l.contains("funnel") && l.contains("not enabled") || l.contains("https") && l.contains("not enabled")) {
                    last = line.trim().to_string();
                }
            }
        }
    }
    kill(child_slot);
    if stop.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    bail!("Tailscale Funnel stopped{}", if last.is_empty() { String::new() } else { format!(": {last}") })
}

fn kill(child_slot: &Arc<Mutex<Option<Child>>>) {
    if let Some(mut c) = child_slot.lock_ok().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        kill(&self.child);
    }
}

/// For tests: is this a program we can run?
#[allow(dead_code)]
fn runnable(p: &Path) -> bool {
    Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_address_in_cloudflareds_output() {
        let lines = [
            "2026-10-18T10:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...",
            "2026-10-18T10:00:01Z INF |  https://mild-sunset-river-lake.trycloudflare.com                                         |",
            "2026-10-18T10:00:01Z INF +--------------------------------------------------------------------------------------------+",
        ];
        let found: Vec<String> = lines.iter().filter_map(|l| address_in(l)).collect();
        assert_eq!(found, vec!["https://mild-sunset-river-lake.trycloudflare.com".to_string()]);
        assert_eq!(address_in("ERR failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\""), None);
        assert!(download_url().unwrap().starts_with("https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-"));
    }

    #[test]
    fn reads_tailscale_funnels_output() {
        let out = "Available on the internet:\n\nhttps://studio-mac.tail1234.ts.net/\n|-- / proxy http://127.0.0.1:8792\n\nPress Ctrl+C to exit.";
        let found: Vec<FunnelLine> = out.lines().map(funnel_line).filter(|l| *l != FunnelLine::Other).collect();
        assert_eq!(found, vec![FunnelLine::Address("https://studio-mac.tail1234.ts.net".into())]);
        assert_eq!(funnel_line("https://studio-mac.tail1234.ts.net:8443/"), FunnelLine::Address("https://studio-mac.tail1234.ts.net".into()));
        let enable = "Funnel is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/funnel?node=nXyZ123CNTRL\n";
        let found: Vec<FunnelLine> = enable.lines().map(funnel_line).filter(|l| *l != FunnelLine::Other).collect();
        assert_eq!(found, vec![FunnelLine::Allow("https://login.tailscale.com/f/funnel?node=nXyZ123CNTRL".into())]);
        assert!(matches!(funnel_line("failed to connect to local Tailscale daemon for /localapi/v0/status; not running?"), FunnelLine::Problem(_)));
        assert!(matches!(funnel_line("Logged out."), FunnelLine::Problem(_)));
        assert_eq!(funnel_line("Press Ctrl+C to exit."), FunnelLine::Other);
    }
}
